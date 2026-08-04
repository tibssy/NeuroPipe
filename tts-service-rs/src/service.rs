use crate::config::Config;
use crate::engines::kokoro::KokoroEngine;
use crate::engines::pocket_tts::PocketTtsEngine;
use crate::engines::{split_sentences, Quality, TtsEngine};
use anyhow::{anyhow, Result};
use rodio::{buffer::SamplesBuffer, OutputStreamBuilder, Sink};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

struct AudioChunk {
    samples: Vec<f32>,
    sample_rate: u32,
    sentence: String,
    generation: u64,
}

struct GenerationRequest {
    text: String,
    voice: String,
    speed: f32,
    generation: u64,
}

pub struct TtsService {
    config: Config,
    engine: Arc<Mutex<Option<Box<dyn TtsEngine>>>>,
    engine_name: Option<String>,
    quality: Option<Quality>,
    generation: Arc<AtomicU64>,
    speaking: Arc<AtomicBool>,
    current_sentence: Arc<Mutex<String>>,
    generating: Arc<AtomicBool>,
    pending_generation: Arc<AtomicUsize>,
    last_activity: Arc<Mutex<Instant>>,
    audio_tx: mpsc::Sender<AudioChunk>,
    audio_rx: Option<mpsc::Receiver<AudioChunk>>,
    generation_tx: mpsc::Sender<GenerationRequest>,
    generation_rx: Option<mpsc::Receiver<GenerationRequest>>,
}

impl TtsService {
    pub fn new(config: Config) -> Self {
        let (audio_tx, audio_rx) = mpsc::channel();
        let (generation_tx, generation_rx) = mpsc::channel();
        Self {
            config,
            engine: Arc::new(Mutex::new(None)),
            engine_name: None,
            quality: None,
            generation: Arc::new(AtomicU64::new(0)),
            speaking: Arc::new(AtomicBool::new(false)),
            current_sentence: Arc::new(Mutex::new(String::new())),
            generating: Arc::new(AtomicBool::new(false)),
            pending_generation: Arc::new(AtomicUsize::new(0)),
            last_activity: Arc::new(Mutex::new(Instant::now())),
            audio_tx,
            audio_rx: Some(audio_rx),
            generation_tx,
            generation_rx: Some(generation_rx),
        }
    }

    pub fn run(mut self) -> Result<()> {
        let receiver = self.audio_rx.take().expect("audio receiver");
        let generation_receiver = self.generation_rx.take().expect("generation receiver");
        let events_addr = self.config.ipc.tts_events.clone();
        let generation = Arc::clone(&self.generation);
        let speaking = Arc::clone(&self.speaking);
        let current_sentence = Arc::clone(&self.current_sentence);
        let last_activity = Arc::clone(&self.last_activity);
        thread::spawn(move || {
            playback_loop(
                receiver,
                events_addr,
                generation,
                speaking,
                current_sentence,
                last_activity,
            )
        });

        let engine = Arc::clone(&self.engine);
        let audio_tx = self.audio_tx.clone();
        let generation = Arc::clone(&self.generation);
        let generating = Arc::clone(&self.generating);
        let pending_generation = Arc::clone(&self.pending_generation);
        thread::spawn(move || {
            generation_loop(
                generation_receiver,
                engine,
                audio_tx,
                generation,
                generating,
                pending_generation,
            )
        });

        let context = zmq::Context::new();
        let command_socket = context.socket(zmq::REP)?;
        command_socket.bind(&self.config.ipc.tts_cmd)?;
        println!("TTS Rust service running on {}", self.config.ipc.tts_cmd);

        let mut poll_items = [command_socket.as_poll_item(zmq::POLLIN)];
        loop {
            zmq::poll(&mut poll_items, 500)?;
            self.release_if_idle();
            if poll_items[0].is_readable() {
                let message: Value = serde_json::from_slice(&command_socket.recv_bytes(0)?)?;
                let response = self.handle(message);
                command_socket.send(response.to_string().as_bytes(), 0)?;
            }
        }
    }

    fn release_if_idle(&mut self) {
        let timeout = Duration::from_secs(self.config.tts.defaults.idle_timeout_sec);
        let idle_for = self
            .last_activity
            .lock()
            .map(|activity| activity.elapsed())
            .unwrap_or_default();
        if idle_for < timeout
            || self.speaking.load(Ordering::SeqCst)
            || self.generating.load(Ordering::SeqCst)
        {
            return;
        }
        let Ok(mut engine_slot) = self.engine.lock() else {
            return;
        };
        if let Some(engine) = engine_slot.as_mut() {
            eprintln!(
                "[TTS] idle for {}s; releasing {} engine",
                self.config.tts.defaults.idle_timeout_sec,
                self.engine_name.as_deref().unwrap_or("unknown")
            );
            engine.unload();
            *engine_slot = None;
            self.engine_name = None;
            self.quality = None;
            if let Ok(mut activity) = self.last_activity.lock() {
                *activity = Instant::now();
            }
            drop(engine_slot);
            trim_process_heap();
        }
    }

    fn touch_activity(&self) {
        if let Ok(mut activity) = self.last_activity.lock() {
            *activity = Instant::now();
        }
    }

    fn handle(&mut self, message: Value) -> Value {
        match message.get("command").and_then(Value::as_str) {
            Some("speak") => self.speak(&message),
            Some("stop") => {
                self.generation.fetch_add(1, Ordering::SeqCst);
                let last_sentence = self
                    .current_sentence
                    .lock()
                    .map(|sentence| sentence.clone())
                    .unwrap_or_default();
                json!({"status": "stopped", "last_sentence": last_sentence})
            }
            Some("warm") => match self.ensure_engine(&message) {
                Ok(()) => json!({"status": "ok"}),
                Err(error) => json!({"status": "error", "message": error.to_string()}),
            },
            Some("get_state") => json!({
                "engine": self.config.tts.defaults.engine,
                "voice": self.config.tts.defaults.voice,
                "speed": self.config.tts.defaults.speed,
                "quality": self.config.tts.defaults.quality,
                "speaking": self.speaking.load(Ordering::SeqCst),
            }),
            Some("list_voices") => self.list_voices(&message),
            Some("set_state") => self.set_state(&message),
            Some(command) => {
                json!({"status": "error", "message": format!("Unknown command '{command}'")})
            }
            None => json!({"status": "error", "message": "Missing command"}),
        }
    }

    fn speak(&mut self, message: &Value) -> Value {
        self.touch_activity();
        let engine = message
            .get("engine")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| self.config.tts.defaults.engine.clone());
        if engine != "kokoro" && engine != "pocket-tts" {
            return json!({"status": "error", "message": "Unsupported Rust TTS engine"});
        }
        let text = match message.get("text").and_then(Value::as_str) {
            Some(text) if !text.trim().is_empty() => text.to_string(),
            _ => return json!({"status": "error", "message": "Missing text"}),
        };
        let voice = message
            .get("voice")
            .and_then(Value::as_str)
            .unwrap_or(&self.config.tts.defaults.voice)
            .to_string();
        let speed = message
            .get("speed")
            .and_then(Value::as_f64)
            .unwrap_or(self.config.tts.defaults.speed as f64) as f32;
        let quality = message
            .get("quality")
            .and_then(Value::as_str)
            .unwrap_or(&self.config.tts.defaults.quality)
            .to_string();
        let quality = match quality.as_str() {
            "low" => Quality::Low,
            "high" => Quality::High,
            _ => return json!({"status": "error", "message": "Invalid quality"}),
        };
        if !(0.5..=2.0).contains(&speed) {
            return json!({"status": "error", "message": "Invalid speed"});
        }

        if let Err(error) = self.ensure_engine_quality(&engine, quality) {
            return json!({"status": "error", "message": error.to_string()});
        }
        if let Err(error) = self.validate_voice(&engine, &voice) {
            return json!({"status": "error", "message": error.to_string()});
        }
        let request_generation = self.generation.load(Ordering::SeqCst);
        self.pending_generation.fetch_add(1, Ordering::SeqCst);
        self.generating.store(true, Ordering::SeqCst);
        if self
            .generation_tx
            .send(GenerationRequest {
                text,
                voice,
                speed,
                generation: request_generation,
            })
            .is_err()
        {
            self.pending_generation.fetch_sub(1, Ordering::SeqCst);
            self.generating.store(false, Ordering::SeqCst);
            return json!({"status": "error", "message": "TTS generation worker stopped"});
        }
        json!({"status": "queued"})
    }

    fn ensure_engine(&mut self, message: &Value) -> Result<()> {
        self.touch_activity();
        let engine = message
            .get("engine")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| self.config.tts.defaults.engine.clone());
        let quality = message
            .get("quality")
            .and_then(Value::as_str)
            .unwrap_or(&self.config.tts.defaults.quality);
        let quality = match quality {
            "low" => Quality::Low,
            "high" => Quality::High,
            _ => return Err(anyhow!("quality must be 'low' or 'high'")),
        };
        self.ensure_engine_quality(&engine, quality)
    }

    fn ensure_engine_quality(&mut self, engine_name: &str, quality: Quality) -> Result<()> {
        if self.engine_name.as_deref() == Some(engine_name) && self.quality == Some(quality) {
            return Ok(());
        }
        let mut guard = self
            .engine
            .lock()
            .map_err(|_| anyhow!("engine lock poisoned"))?;
        if let Some(engine) = guard.as_mut() {
            if self.engine_name.as_deref() != Some(engine_name) {
                *guard = None;
            } else {
                if self.quality != Some(quality) {
                    engine.set_quality(quality)?;
                    self.quality = Some(quality);
                }
                engine.load()?;
                return Ok(());
            }
        }
        let path = model_path(engine_name);
        let mut engine: Box<dyn TtsEngine> = match engine_name {
            "kokoro" => Box::new(KokoroEngine::new(path, quality)),
            "pocket-tts" => Box::new(PocketTtsEngine::new(path, quality)),
            _ => return Err(anyhow!("unsupported Rust TTS engine '{engine_name}'")),
        };
        engine.load()?;
        *guard = Some(engine);
        self.engine_name = Some(engine_name.to_string());
        self.quality = Some(quality);
        Ok(())
    }

    fn list_voices(&mut self, message: &Value) -> Value {
        let engine_name = message
            .get("engine")
            .and_then(Value::as_str)
            .unwrap_or(&self.config.tts.defaults.engine)
            .to_string();
        match self.available_voices(&engine_name) {
            Ok(voices) => json!({"voices": voices}),
            Err(error) => json!({"voices": [], "status": "error", "message": error.to_string()}),
        }
    }

    fn available_voices(&mut self, engine_name: &str) -> Result<Vec<String>> {
        let mut engine: Box<dyn TtsEngine> = match engine_name {
            "kokoro" => Box::new(KokoroEngine::new(model_path(engine_name), Quality::High)),
            "pocket-tts" => Box::new(PocketTtsEngine::new(model_path(engine_name), Quality::High)),
            _ => return Err(anyhow!("unsupported Rust TTS engine '{engine_name}'")),
        };
        engine.voices()
    }

    fn validate_voice(&mut self, engine_name: &str, voice: &str) -> Result<()> {
        if voice.ends_with(".safetensors") {
            if engine_name != "pocket-tts" {
                return Err(anyhow!("custom .safetensors voices require pocket-tts"));
            }
            let path = expand_path(voice);
            if !path.is_file() {
                return Err(anyhow!("voice file not found: {}", path.display()));
            }
            return Ok(());
        }
        if self
            .available_voices(engine_name)?
            .iter()
            .any(|available| available == voice)
        {
            Ok(())
        } else {
            Err(anyhow!(
                "voice '{voice}' is not available for engine '{engine_name}'"
            ))
        }
    }

    fn set_state(&mut self, message: &Value) -> Value {
        self.touch_activity();
        let engine = message
            .get("engine")
            .and_then(Value::as_str)
            .unwrap_or(&self.config.tts.defaults.engine)
            .to_string();
        let voice = message
            .get("voice")
            .and_then(Value::as_str)
            .unwrap_or(&self.config.tts.defaults.voice)
            .to_string();
        let speed = message
            .get("speed")
            .and_then(Value::as_f64)
            .unwrap_or(self.config.tts.defaults.speed as f64) as f32;
        let quality = message
            .get("quality")
            .and_then(Value::as_str)
            .unwrap_or(&self.config.tts.defaults.quality)
            .to_string();
        if !matches!(quality.as_str(), "low" | "high") {
            return json!({"status": "error", "message": "Invalid quality"});
        }
        if engine != "kokoro" && engine != "pocket-tts" {
            return json!({"status": "error", "message": "Unsupported Rust TTS engine"});
        }
        if !(0.5..=2.0).contains(&speed) {
            return json!({"status": "error", "message": "Invalid speed"});
        }
        if let Err(error) = self.validate_voice(&engine, &voice) {
            return json!({"status": "error", "message": error.to_string()});
        }
        self.config.tts.defaults.engine = engine;
        self.config.tts.defaults.voice = voice;
        self.config.tts.defaults.speed = speed;
        self.config.tts.defaults.quality = quality;
        json!({"status": "ok", "engine": self.config.tts.defaults.engine, "voice": self.config.tts.defaults.voice, "speed": self.config.tts.defaults.speed, "quality": self.config.tts.defaults.quality})
    }
}

fn model_path(engine: &str) -> PathBuf {
    let suffix = match engine {
        "pocket-tts" => ".local/share/neuropipe/models/pocket-tts",
        _ => ".local/share/neuropipe/models/kokoro",
    };
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(suffix))
        .unwrap_or_else(|| PathBuf::from(format!("~/{suffix}")))
}

fn expand_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(target_os = "linux")]
fn trim_process_heap() {
    // ONNX Runtime sessions release their allocations, but glibc may retain
    // free pages in the process heap instead of returning them to the kernel.
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(not(target_os = "linux"))]
fn trim_process_heap() {}

fn generation_loop(
    receiver: mpsc::Receiver<GenerationRequest>,
    engine: Arc<Mutex<Option<Box<dyn TtsEngine>>>>,
    audio_tx: mpsc::Sender<AudioChunk>,
    generation: Arc<AtomicU64>,
    generating: Arc<AtomicBool>,
    pending_generation: Arc<AtomicUsize>,
) {
    for request in receiver {
        if generation.load(Ordering::SeqCst) == request.generation {
            if let Ok(mut guard) = engine.lock() {
                if let Some(engine) = guard.as_mut() {
                    for sentence in split_sentences(&request.text) {
                        match engine.synthesize(&sentence, &request.voice, request.speed) {
                            Ok((samples, sample_rate))
                                if generation.load(Ordering::SeqCst) == request.generation =>
                            {
                                eprintln!(
                                    "[TTS] generated {} samples for '{}'; queueing",
                                    samples.len(),
                                    sentence
                                );
                                let _ = audio_tx.send(AudioChunk {
                                    samples,
                                    sample_rate,
                                    sentence,
                                    generation: request.generation,
                                });
                            }
                            Ok(_) => break,
                            Err(error) => {
                                eprintln!("[TTS] generation error: {error:#}");
                                break;
                            }
                        }
                    }
                }
            }
        }
        if pending_generation.fetch_sub(1, Ordering::SeqCst) == 1 {
            generating.store(false, Ordering::SeqCst);
        }
    }
}

fn playback_loop(
    receiver: mpsc::Receiver<AudioChunk>,
    events_addr: String,
    generation: Arc<AtomicU64>,
    speaking: Arc<AtomicBool>,
    current_sentence: Arc<Mutex<String>>,
    last_activity: Arc<Mutex<Instant>>,
) {
    let Ok(stream) = OutputStreamBuilder::open_default_stream() else {
        eprintln!("[TTS] no default audio output available");
        return;
    };
    let context = zmq::Context::new();
    let Ok(events) = context.socket(zmq::PUB) else {
        return;
    };
    if events.bind(&events_addr).is_err() {
        return;
    }
    for chunk in receiver {
        if chunk.generation != generation.load(Ordering::SeqCst) {
            continue;
        }
        speaking.store(true, Ordering::SeqCst);
        if let Ok(mut current) = current_sentence.lock() {
            *current = chunk.sentence.clone();
        }
        let _ = events.send(
            json!({"event": "speaking", "sentence": chunk.sentence})
                .to_string()
                .as_bytes(),
            0,
        );
        let sink = Sink::connect_new(stream.mixer());
        sink.append(SamplesBuffer::new(1, chunk.sample_rate, chunk.samples));
        while !sink.empty() {
            if generation.load(Ordering::SeqCst) != chunk.generation {
                sink.stop();
                let _ = events.send(
                    json!({"event": "interrupted", "last_sentence": chunk.sentence})
                        .to_string()
                        .as_bytes(),
                    0,
                );
                break;
            }
            thread::sleep(std::time::Duration::from_millis(20));
        }
        if generation.load(Ordering::SeqCst) == chunk.generation {
            let _ = events.send(
                json!({"event": "sentence_done", "sentence": chunk.sentence})
                    .to_string()
                    .as_bytes(),
                0,
            );
        }
        speaking.store(false, Ordering::SeqCst);
        if let Ok(mut current) = current_sentence.lock() {
            current.clear();
        }
        if let Ok(mut activity) = last_activity.lock() {
            *activity = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::Quality;

    struct FakeEngine;

    impl TtsEngine for FakeEngine {
        fn load(&mut self) -> Result<()> {
            Ok(())
        }

        fn unload(&mut self) {}

        fn set_quality(&mut self, _quality: Quality) -> Result<()> {
            Ok(())
        }

        fn voices(&mut self) -> Result<Vec<String>> {
            Ok(vec!["test".to_string()])
        }

        fn synthesize(&mut self, text: &str, _voice: &str, _speed: f32) -> Result<(Vec<f32>, u32)> {
            Ok((vec![text.len() as f32], 24_000))
        }
    }

    #[test]
    fn generation_worker_preserves_fifo_order() {
        let (request_tx, request_rx) = mpsc::channel();
        let (audio_tx, audio_rx) = mpsc::channel();
        let engine: Arc<Mutex<Option<Box<dyn TtsEngine>>>> =
            Arc::new(Mutex::new(Some(Box::new(FakeEngine))));
        let generation = Arc::new(AtomicU64::new(0));
        let generating = Arc::new(AtomicBool::new(true));
        let pending = Arc::new(AtomicUsize::new(2));

        let worker = thread::spawn({
            let engine = Arc::clone(&engine);
            let generation = Arc::clone(&generation);
            let generating = Arc::clone(&generating);
            let pending = Arc::clone(&pending);
            move || {
                generation_loop(
                    request_rx, engine, audio_tx, generation, generating, pending,
                )
            }
        });

        request_tx
            .send(GenerationRequest {
                text: "first".to_string(),
                voice: "test".to_string(),
                speed: 1.0,
                generation: 0,
            })
            .unwrap();
        request_tx
            .send(GenerationRequest {
                text: "second".to_string(),
                voice: "test".to_string(),
                speed: 1.0,
                generation: 0,
            })
            .unwrap();
        drop(request_tx);
        worker.join().unwrap();

        let sentences = audio_rx
            .into_iter()
            .map(|chunk| chunk.sentence)
            .collect::<Vec<_>>();
        assert_eq!(sentences, vec!["first", "second"]);
        assert!(!generating.load(Ordering::SeqCst));
        assert_eq!(pending.load(Ordering::SeqCst), 0);
    }
}
