use crate::audio::{MicInput, WINDOW_SIZE};
use crate::config::Config;
use crate::engines::endpoint::{HeuristicTurnEnd, TurnContext};
use crate::engines::{parakeet::ParakeetEngine, SttEngine, TurnEndDetector};
use crate::vad::Vad;
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub const SAMPLE_RATE: u32 = 16_000;

const PRE_RECORD_MS: u64 = 500;
const MAX_RECORDING_SECONDS: u64 = 15;
const CHUNKS_PER_SEC: f64 = SAMPLE_RATE as f64 / WINDOW_SIZE as f64;
const PRE_RECORD_CHUNKS: usize = (PRE_RECORD_MS as f64 / 1000.0 * CHUNKS_PER_SEC) as usize;
const MAX_RECORDING_CHUNKS: usize = (MAX_RECORDING_SECONDS as f64 * CHUNKS_PER_SEC) as usize;
/// One 512-sample frame at 16 kHz is 32 ms of audio.
const CHUNK_MS: u64 = WINDOW_SIZE as u64 * 1000 / SAMPLE_RATE as u64;
/// Rolling audio window handed to the turn-end detector on each score pass.
const TAIL_SECS: f64 = 1.8;
const TAIL_SAMPLES: usize = (SAMPLE_RATE as f64 * TAIL_SECS) as usize;

/// Mutable state of the current (or idle) VAD recording session.
struct Recording {
    pre_speech: VecDeque<Vec<f32>>,
    recorded: Vec<Vec<f32>>,
    tail: VecDeque<f32>,
    is_recording: bool,
    silence_ms: u64,
    last_scored_ms: u64,
}

impl Recording {
    fn new() -> Self {
        Self {
            pre_speech: VecDeque::with_capacity(PRE_RECORD_CHUNKS),
            recorded: Vec::new(),
            tail: VecDeque::with_capacity(TAIL_SAMPLES),
            is_recording: false,
            silence_ms: 0,
            last_scored_ms: 0,
        }
    }

    /// Buffer a chunk while idle (keeps the pre-roll ring fresh).
    fn buffer_pre(&mut self, chunk: Vec<f32>) {
        if self.pre_speech.len() == PRE_RECORD_CHUNKS {
            self.pre_speech.pop_front();
        }
        self.pre_speech.push_back(chunk);
    }

    /// Start recording by moving the pre-roll into the buffer.
    fn start(&mut self) {
        self.recorded.extend(self.pre_speech.drain(..));
        self.silence_ms = 0;
        self.last_scored_ms = 0;
        self.is_recording = true;
    }

    /// Append a chunk while recording and maintain the rolling tail window.
    fn push(&mut self, chunk: &[f32]) {
        self.recorded.push(chunk.to_vec());
        for &sample in chunk {
            self.tail.push_back(sample);
        }
        while self.tail.len() > TAIL_SAMPLES {
            self.tail.pop_front();
        }
    }

    /// A speech frame was seen: the current silence run is over.
    fn on_speech(&mut self) {
        self.silence_ms = 0;
        self.last_scored_ms = 0;
    }

    fn utterance_ms(&self) -> u64 {
        self.recorded.iter().map(|c| c.len() as u64).sum::<u64>() * 1000 / SAMPLE_RATE as u64
    }

    fn tail_samples(&self) -> Vec<f32> {
        self.tail.iter().copied().collect()
    }

    fn take_recorded(&mut self) -> Vec<f32> {
        self.recorded.concat()
    }

    fn reset(&mut self) {
        self.pre_speech.clear();
        self.recorded.clear();
        self.tail.clear();
        self.is_recording = false;
        self.silence_ms = 0;
        self.last_scored_ms = 0;
    }
}

pub struct SttService {
    config: Config,
    engine: Option<ParakeetEngine>,
    vad: Option<Vad>,
    endpoint: Option<Box<dyn TurnEndDetector>>,
    last_activity: Arc<Mutex<Instant>>,
}

impl SttService {
    pub fn new(config: Config) -> Self {
        let model_dir = config.stt_model_dir();
        Self {
            config,
            engine: Some(ParakeetEngine::new(model_dir)),
            vad: None,
            endpoint: Some(Box::new(HeuristicTurnEnd::new())),
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
            .spawn(move || {
                transcription_worker(job_rx, result_tx, engine, last_activity, idle_timeout)
            })?;

        let context = zmq::Context::new();
        let pub_sock = context.socket(zmq::PUB)?;
        pub_sock.bind(&self.config.ipc.stt_pub)?;
        let rep_sock = context.socket(zmq::REP)?;
        rep_sock.bind(&self.config.ipc.stt_cmd)?;
        println!("STT Rust service running on {}", self.config.ipc.stt_cmd);

        let mut mode = initial_mode;
        let mut rec = Recording::new();

        let mut poll_items = [rep_sock.as_poll_item(zmq::POLLIN)];
        loop {
            zmq::poll(&mut poll_items, 25)?;

            if poll_items[0].is_readable() {
                let message: Value = serde_json::from_slice(&rep_sock.recv_bytes(0)?)?;
                let response =
                    self.handle_command(&message, &mut mode, &mut rec, &pub_sock, &job_tx);
                rep_sock.send(response.to_string().as_bytes(), 0)?;
            }

            // Publish completed transcriptions from the worker thread.
            while let Ok(text) = result_rx.try_recv() {
                eprintln!("[STT] > {text}");
                let _ = pub_sock.send(
                    json!({"event": "transcription", "text": text})
                        .to_string()
                        .as_bytes(),
                    0,
                );
            }

            if mode == "IDLE" {
                // Keep the always-on microphone queue fresh while inactive.
                // Otherwise stale audio is processed by the next trigger.
                while audio_rx.try_recv().is_ok() {}
            } else {
                match audio_rx.try_recv() {
                    Ok(mut chunk) => {
                        self.apply_gain(&mut chunk);
                        match mode.as_str() {
                            "VAD" => {
                                self.process_vad(chunk, &mut rec, &pub_sock, &job_tx);
                            }
                            "MANUAL" => rec.recorded.push(chunk),
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
        rec: &mut Recording,
        pub_sock: &zmq::Socket,
        job_tx: &mpsc::Sender<Vec<f32>>,
    ) {
        let threshold = self.config.stt.vad_threshold;
        let prob = self
            .vad
            .as_mut()
            .and_then(|v| v.predict(&chunk).ok())
            .unwrap_or(0.0);

        if !rec.is_recording {
            rec.buffer_pre(chunk);
            if prob > threshold {
                rec.start();
                eprintln!("[STT] VAD start");
                let _ = pub_sock.send(
                    json!({"event": "listening_start"}).to_string().as_bytes(),
                    0,
                );
            }
            return;
        }

        rec.push(&chunk);
        if prob >= threshold {
            rec.on_speech();
            return;
        }

        rec.silence_ms += CHUNK_MS;
        if rec.recorded.len() > MAX_RECORDING_CHUNKS || self.should_end_turn(rec, prob) {
            self.end_turn(rec, pub_sock, job_tx);
        }
    }

    /// Decide whether the current pause marks the end of the turn. When
    /// `turn_end_enabled`, a small detector scores the pause (after a hold
    /// window, re-scored on a cadence) and a hard ceiling always finalizes.
    /// Otherwise falls back to the legacy fixed `silence_timeout_sec`.
    fn should_end_turn(&mut self, rec: &mut Recording, prob: f32) -> bool {
        let cfg = &self.config.stt;
        if cfg.turn_end_enabled {
            if rec.silence_ms >= cfg.turn_hard_ceiling_ms {
                return true;
            }
            if rec.silence_ms >= cfg.turn_hold_ms
                && rec.silence_ms - rec.last_scored_ms >= cfg.turn_score_cadence_ms
            {
                let ctx = TurnContext {
                    tail: rec.tail_samples(),
                    silence_ms: rec.silence_ms,
                    utterance_ms: rec.utterance_ms(),
                    last_vad: prob,
                };
                let score = self
                    .endpoint
                    .as_mut()
                    .map(|detector| detector.score(&ctx))
                    .unwrap_or(0.0);
                eprintln!("[STT] turn score={score:.3} silence={}ms", rec.silence_ms);
                rec.last_scored_ms = rec.silence_ms;
                return score > cfg.turn_end_threshold;
            }
            false
        } else {
            let max_silence_ms = (cfg.silence_timeout_sec as f64 * 1000.0) as u64;
            rec.silence_ms >= max_silence_ms
        }
    }

    fn end_turn(
        &mut self,
        rec: &mut Recording,
        pub_sock: &zmq::Socket,
        job_tx: &mpsc::Sender<Vec<f32>>,
    ) {
        eprintln!("[STT] Processing...");
        let full = rec.take_recorded();
        rec.reset();
        if let Some(v) = self.vad.as_mut() {
            v.reset();
        }
        if let Some(detector) = self.endpoint.as_mut() {
            detector.reset();
        }
        let _ = pub_sock.send(json!({"event": "listening_end"}).to_string().as_bytes(), 0);
        touch_activity(&self.last_activity);
        let _ = job_tx.send(full);
    }

    fn handle_command(
        &mut self,
        message: &Value,
        mode: &mut String,
        rec: &mut Recording,
        pub_sock: &zmq::Socket,
        job_tx: &mpsc::Sender<Vec<f32>>,
    ) -> Value {
        match message.get("command").and_then(Value::as_str) {
            Some("get_state") => json!({
                "mode": mode,
                "vad_threshold": self.config.stt.vad_threshold,
                "silence_timeout_sec": self.config.stt.silence_timeout_sec,
                "turn_end_enabled": self.config.stt.turn_end_enabled,
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
                rec.reset();
                if *mode == "VAD" {
                    if let Some(v) = self.vad.as_mut() {
                        v.reset();
                    }
                }
                let _ = pub_sock.send(
                    json!({"event": "mode_changed", "mode": mode})
                        .to_string()
                        .as_bytes(),
                    0,
                );
                touch_activity(&self.last_activity);
                json!({"status": "ok"})
            }
            Some("manual_stop") => {
                if !rec.recorded.is_empty() {
                    let full = rec.take_recorded();
                    touch_activity(&self.last_activity);
                    let _ = job_tx.send(full);
                }
                *mode = "IDLE".to_string();
                rec.reset();
                touch_activity(&self.last_activity);
                json!({"status": "ok"})
            }
            Some(command) => {
                json!({"status": "error", "message": format!("Unknown command '{command}'")})
            }
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
                    eprintln!(
                        "[STT] idle for {}s; unloading model",
                        idle_timeout.as_secs()
                    );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(sample: f32) -> Vec<f32> {
        vec![sample; WINDOW_SIZE]
    }

    #[test]
    fn recording_pre_roll_ring_is_bounded() {
        let mut rec = Recording::new();
        for i in 0..PRE_RECORD_CHUNKS + 5 {
            rec.buffer_pre(chunk(i as f32));
        }
        assert_eq!(rec.pre_speech.len(), PRE_RECORD_CHUNKS);
    }

    #[test]
    fn recording_tail_is_bounded() {
        let mut rec = Recording::new();
        rec.start();
        // Feed far more audio than the tail window.
        for _ in 0..(TAIL_SAMPLES / WINDOW_SIZE + 50) {
            rec.push(&chunk(0.1));
        }
        assert!(rec.tail.len() <= TAIL_SAMPLES);
    }

    #[test]
    fn recording_utterance_ms_tracks_samples() {
        let mut rec = Recording::new();
        rec.start();
        for _ in 0..100 {
            rec.push(&chunk(0.1));
        }
        // 100 chunks * 512 samples = 51200 samples = 3.2 s.
        assert_eq!(rec.utterance_ms(), 3200);
    }

    #[test]
    fn silence_run_resets_on_speech() {
        let mut rec = Recording::new();
        rec.start();
        rec.silence_ms = 500;
        rec.last_scored_ms = 300;
        rec.on_speech();
        assert_eq!(rec.silence_ms, 0);
        assert_eq!(rec.last_scored_ms, 0);
    }

    fn tail_glide(freq_from: f32, freq_to: f32) -> Vec<f32> {
        let sr = 16_000.0f32;
        let n = (0.6 * sr) as usize;
        let mut out = Vec::with_capacity(n);
        let mut phase = 0.0f32;
        for i in 0..n {
            let frac = i as f32 / n as f32;
            let freq = freq_from + (freq_to - freq_from) * frac;
            out.push(phase.sin());
            phase += 2.0 * std::f32::consts::PI * freq / sr;
        }
        out
    }

    fn test_service(config: Config) -> SttService {
        SttService::new(config)
    }

    #[test]
    fn endpoint_holds_below_hold_window() {
        let mut svc = test_service(Config::default());
        let mut rec = Recording::new();
        rec.start();
        rec.push(&tail_glide(440.0, 160.0));
        rec.silence_ms = 100; // below turn_hold_ms (250)
        assert!(!svc.should_end_turn(&mut rec, 0.0));
    }

    #[test]
    fn endpoint_finalizes_at_hard_ceiling() {
        let mut svc = test_service(Config::default());
        let mut rec = Recording::new();
        rec.start();
        rec.push(&tail_glide(440.0, 220.0));
        rec.silence_ms = 3000; // >= turn_hard_ceiling_ms (2500)
        assert!(svc.should_end_turn(&mut rec, 0.0));
    }

    #[test]
    fn endpoint_keeps_recording_on_flat_contour() {
        let mut svc = test_service(Config::default());
        let mut rec = Recording::new();
        rec.start();
        rec.push(&tail_glide(220.0, 220.0)); // flat => continuation
        rec.silence_ms = 700;
        assert!(!svc.should_end_turn(&mut rec, 0.0));
    }

    #[test]
    fn endpoint_finalizes_early_on_terminal_contour() {
        let mut svc = test_service(Config::default());
        let mut rec = Recording::new();
        rec.start();
        rec.push(&tail_glide(440.0, 160.0)); // falling => terminal
        rec.silence_ms = 700;
        assert!(svc.should_end_turn(&mut rec, 0.0));
    }

    #[test]
    fn endpoint_respects_score_cadence() {
        let mut svc = test_service(Config::default());
        let mut rec = Recording::new();
        rec.start();
        rec.push(&tail_glide(440.0, 160.0));
        rec.silence_ms = 600;
        assert!(svc.should_end_turn(&mut rec, 0.0)); // first score fires
        rec.silence_ms = 632; // only 32ms later, under cadence (400)
        assert!(!svc.should_end_turn(&mut rec, 0.0));
    }

    #[test]
    fn endpoint_falls_back_to_fixed_silence_when_disabled() {
        let stt = crate::config::SttConfig {
            mode: "VAD".into(),
            model: "m".into(),
            model_dir: "d".into(),
            vad_threshold: 0.5,
            digital_gain: 1.0,
            silence_timeout_sec: 1.0,
            model_idle_timeout_sec: 60,
            turn_end_enabled: false,
            turn_hold_ms: 250,
            turn_end_threshold: 0.5,
            turn_score_cadence_ms: 400,
            turn_hard_ceiling_ms: 2500,
        };
        let config = crate::config::Config {
            ipc: crate::config::IpcConfig {
                stt_cmd: "ipc:///tmp/t.sock".into(),
                stt_pub: "ipc:///tmp/t.sock".into(),
            },
            stt,
        };
        let mut svc = test_service(config);
        let mut rec = Recording::new();
        rec.start();
        rec.push(&tail_glide(440.0, 160.0));
        rec.silence_ms = 500; // under 1s fixed timeout
        assert!(!svc.should_end_turn(&mut rec, 0.0));
        rec.silence_ms = 1024; // over 1s fixed timeout
        assert!(svc.should_end_turn(&mut rec, 0.0));
    }
}
