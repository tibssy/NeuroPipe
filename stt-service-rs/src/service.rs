use crate::audio::{MicInput, WINDOW_SIZE};
use crate::config::Config;
use crate::engines::{parakeet::ParakeetEngine, SttEngine};
use crate::vad::Vad;
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub const SAMPLE_RATE: u32 = 16_000;

const SILENCE_DURATION_MS: u64 = 1000;
const PRE_RECORD_MS: u64 = 500;
const MAX_RECORDING_SECONDS: u64 = 15;
const CHUNKS_PER_SEC: f64 = SAMPLE_RATE as f64 / WINDOW_SIZE as f64;
const MAX_SILENCE_CHUNKS: usize = (SILENCE_DURATION_MS as f64 / 1000.0 * CHUNKS_PER_SEC) as usize;
const PRE_RECORD_CHUNKS: usize = (PRE_RECORD_MS as f64 / 1000.0 * CHUNKS_PER_SEC) as usize;
const MAX_RECORDING_CHUNKS: usize = (MAX_RECORDING_SECONDS as f64 * CHUNKS_PER_SEC) as usize;

pub struct SttService {
    config: Config,
    engine: Option<ParakeetEngine>,
    vad: Option<Vad>,
    last_activity: Arc<Mutex<Instant>>,
}

impl SttService {
    pub fn new(config: Config) -> Self {
        let stt = config.stt.clone();
        let model_dir = config.stt_model_dir();
        Self {
            config,
            engine: Some(ParakeetEngine::new(model_dir, stt.quantization.clone())),
            vad: None,
            last_activity: Arc::new(Mutex::new(Instant::now())),
        }
    }

    pub fn run(&mut self) -> Result<()> {
        let initial_mode = self.config.stt.mode.clone();

        let (audio_tx, audio_rx) = mpsc::channel();
        let _mic;
        if let Some(wav) = std::env::var_os("FAKE_MIC") {
            crate::audio::FakeMic::open(wav, audio_tx)?;
        } else {
            _mic = MicInput::open(audio_tx)?;
        }

        let vad_path = self.config.vad_path();
        self.ensure_vad(&vad_path)?;

        // Transcribe in a background worker (like the legacy Python service) so
        // the audio/VAD loop is never blocked by ASR inference.
        let engine = Arc::new(Mutex::new(self.engine.take().expect("engine")));
        let (job_tx, job_rx) = mpsc::channel::<Vec<f32>>();
        let (result_tx, result_rx) = mpsc::channel::<String>();
        let idle_timeout = Duration::from_secs(self.config.stt.model_idle_timeout_sec);
        let last_activity = Arc::clone(&self.last_activity);
        std::thread::Builder::new()
            .name("stt-transcriber".to_string())
            .spawn(move || transcription_worker(job_rx, result_tx, engine, last_activity, idle_timeout))?;

        let context = zmq::Context::new();
        let pub_sock = context.socket(zmq::PUB)?;
        pub_sock.bind(&self.config.ipc.stt_pub)?;
        let rep_sock = context.socket(zmq::REP)?;
        rep_sock.bind(&self.config.ipc.stt_cmd)?;
        println!("STT Rust service running on {}", self.config.ipc.stt_cmd);

        let mut mode = initial_mode;
        let mut pre_speech: VecDeque<Vec<f32>> = VecDeque::with_capacity(PRE_RECORD_CHUNKS);
        let mut recorded: Vec<Vec<f32>> = Vec::new();
        let mut is_recording = false;
        let mut silence_counter = 0usize;

        let mut poll_items = [rep_sock.as_poll_item(zmq::POLLIN)];
        loop {
            zmq::poll(&mut poll_items, 25)?;

            if poll_items[0].is_readable() {
                let message: Value = serde_json::from_slice(&rep_sock.recv_bytes(0)?)?;
                let response = self.handle_command(
                    &message,
                    &mut mode,
                    &mut pre_speech,
                    &mut recorded,
                    &mut is_recording,
                    &pub_sock,
                    &job_tx,
                );
                rep_sock.send(response.to_string().as_bytes(), 0)?;
            }

            // Publish completed transcriptions from the worker thread.
            while let Ok(text) = result_rx.try_recv() {
                eprintln!("[STT] > {text}");
                let _ = pub_sock.send(
                    json!({"event": "transcription", "text": text}).to_string().as_bytes(),
                    0,
                );
            }

            if mode != "IDLE" {
                match audio_rx.try_recv() {
                    Ok(mut chunk) => {
                        self.apply_gain(&mut chunk);
                        match mode.as_str() {
                            "VAD" => {
                                self.process_vad(
                                    chunk,
                                    &mut pre_speech,
                                    &mut recorded,
                                    &mut is_recording,
                                    &mut silence_counter,
                                    &pub_sock,
                                    &job_tx,
                                );
                            }
                            "MANUAL" => recorded.push(chunk),
                            _ => {}
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => break,
                }
            }
        }
        #[allow(unreachable_code)]
        Ok(())
    }

    fn process_vad(
        &mut self,
        chunk: Vec<f32>,
        pre_speech: &mut VecDeque<Vec<f32>>,
        recorded: &mut Vec<Vec<f32>>,
        is_recording: &mut bool,
        silence_counter: &mut usize,
        pub_sock: &zmq::Socket,
        job_tx: &mpsc::Sender<Vec<f32>>,
    ) {
        let threshold = self.config.stt.vad_threshold;
        let prob = self
            .vad
            .as_mut()
            .and_then(|v| v.predict(&chunk).ok())
            .unwrap_or(0.0);

        if !*is_recording {
            if pre_speech.len() == PRE_RECORD_CHUNKS {
                pre_speech.pop_front();
            }
            pre_speech.push_back(chunk);
            if prob > threshold {
                *is_recording = true;
                eprintln!("[STT] VAD start");
                let _ = pub_sock.send(
                    json!({"event": "listening_start"}).to_string().as_bytes(),
                    0,
                );
                recorded.extend(pre_speech.drain(..));
                *silence_counter = 0;
            }
        } else {
            recorded.push(chunk);
            if prob < threshold {
                *silence_counter += 1;
            } else {
                *silence_counter = 0;
            }
            if *silence_counter > MAX_SILENCE_CHUNKS || recorded.len() > MAX_RECORDING_CHUNKS {
                eprintln!("[STT] Processing...");
                let full = recorded.concat();
                recorded.clear();
                *is_recording = false;
                *silence_counter = 0;
                if let Some(v) = self.vad.as_mut() {
                    v.reset();
                }
                let _ = pub_sock.send(
                    json!({"event": "listening_end"}).to_string().as_bytes(),
                    0,
                );
                touch_activity(&self.last_activity);
                let _ = job_tx.send(full);
            }
        }
    }

    fn handle_command(
        &mut self,
        message: &Value,
        mode: &mut String,
        pre_speech: &mut VecDeque<Vec<f32>>,
        recorded: &mut Vec<Vec<f32>>,
        is_recording: &mut bool,
        pub_sock: &zmq::Socket,
        job_tx: &mpsc::Sender<Vec<f32>>,
    ) -> Value {
        match message.get("command").and_then(Value::as_str) {
            Some("get_state") => json!({
                "mode": mode,
                "vad_threshold": self.config.stt.vad_threshold,
                "sample_rate": SAMPLE_RATE,
                "model": self.config.stt.model,
            }),
            Some("set_mode") => {
                let new_mode = message
                    .get("mode")
                    .and_then(Value::as_str)
                    .unwrap_or("IDLE")
                    .to_string();
                *mode = new_mode;
                *is_recording = false;
                recorded.clear();
                pre_speech.clear();
                if *mode == "VAD" {
                    if let Some(v) = self.vad.as_mut() {
                        v.reset();
                    }
                }
                let _ = pub_sock.send(
                    json!({"event": "mode_changed", "mode": mode}).to_string().as_bytes(),
                    0,
                );
                touch_activity(&self.last_activity);
                json!({"status": "ok"})
            }
            Some("manual_stop") => {
                if !recorded.is_empty() {
                    let full = std::mem::take(recorded).concat();
                    touch_activity(&self.last_activity);
                    let _ = job_tx.send(full);
                }
                *mode = "IDLE".to_string();
                *is_recording = false;
                recorded.clear();
                pre_speech.clear();
                touch_activity(&self.last_activity);
                json!({"status": "ok"})
            }
            Some(command) => json!({"status": "error", "message": format!("Unknown command '{command}'")}),
            None => json!({"status": "error", "message": "Missing command"}),
        }
    }

    fn apply_gain(&self, chunk: &mut [f32]) {
        let gain = self.config.stt.digital_gain;
        if gain == 1.0 {
            return;
        }
        for v in chunk.iter_mut() {
            *v = (*v * gain).clamp(-1.0, 1.0);
        }
    }

    fn ensure_vad(&mut self, path: &std::path::Path) -> Result<()> {
        self.vad = Some(Vad::load(path)?);
        Ok(())
    }
}

fn touch_activity(last_activity: &Arc<Mutex<Instant>>) {
    if let Ok(mut a) = last_activity.lock() {
        *a = Instant::now();
    }
}

/// Background thread: transcribe queued audio and unload the ASR model when
/// idle, so the audio loop never blocks on inference.
fn transcription_worker(
    job_rx: mpsc::Receiver<Vec<f32>>,
    result_tx: mpsc::Sender<String>,
    engine: Arc<Mutex<ParakeetEngine>>,
    last_activity: Arc<Mutex<Instant>>,
    idle_timeout: Duration,
) {
    loop {
        match job_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(samples) => {
                let text = {
                    let mut engine = engine.lock().unwrap();
                    engine.transcribe(&samples)
                }
                .map(|t| t.trim().to_string())
                .unwrap_or_else(|error| {
                    eprintln!("[STT] transcription error: {error:#}");
                    String::new()
                });
                touch_activity(&last_activity);
                if !text.is_empty() {
                    let _ = result_tx.send(text);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let idle_for = last_activity
                    .lock()
                    .map(|a| a.elapsed())
                    .unwrap_or_default();
                if idle_for < idle_timeout {
                    continue;
                }
                let should_unload = {
                    let engine = engine.lock().unwrap();
                    engine.is_loaded()
                };
                if should_unload {
                    eprintln!("[STT] idle for {}s; unloading model", idle_timeout.as_secs());
                    {
                        let mut engine = engine.lock().unwrap();
                        engine.unload();
                    }
                    touch_activity(&last_activity);
                    trim_process_heap();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(target_os = "linux")]
fn trim_process_heap() {
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(not(target_os = "linux"))]
fn trim_process_heap() {}