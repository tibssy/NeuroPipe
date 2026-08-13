mod audio;
mod config;
mod engines;
mod service;
mod vad;

use anyhow::{bail, Context, Result};
use engines::SttEngine;
use engines::TurnEndDetector;
use std::collections::VecDeque;
use std::env;
use std::path::Path;

fn main() -> Result<()> {
    let raw_args = env::args().skip(1).collect::<Vec<_>>();

    if let Some(pos) = raw_args.iter().position(|a| a == "--transcribe") {
        let wav = raw_args.get(pos + 1).ok_or_else(|| anyhow::anyhow!("--transcribe needs a wav path"))?;
        let cfg = config::load();
        let model_dir = cfg.stt_model_dir();
        let mut engine = engines::parakeet::ParakeetEngine::new(&model_dir);
        engine.load()?;
        let samples = read_wav_f32(wav)?;
        let text = engine.transcribe(&samples)?;
        println!("{text}");
        return Ok(());
    }

    if let Some(pos) = raw_args.iter().position(|a| a == "--vad-file") {
        let wav = raw_args.get(pos + 1).ok_or_else(|| anyhow::anyhow!("--vad-file needs a wav path"))?;
        let cfg = config::load();
        let mut vad = vad::Vad::load(cfg.vad_path())?;
        let samples = read_wav_f32(wav)?;
        let mut best = 0.0f32;
        let mut n = 0usize;
        let mut wins = 0usize;
        for frame in samples.chunks(512) {
            let mut f = vec![0.0f32; 512];
            f[..frame.len()].copy_from_slice(frame);
            let p = vad.predict(&f)?;
            best = best.max(p);
            if p > 0.1 {
                wins += 1;
            }
            n += 1;
        }
        println!("frames={n} best_vad={best:.4} frames_over_0.1={wins}");
        return Ok(());
    }

    if let Some(pos) = raw_args.iter().position(|a| a == "--debug-mic") {
        let seconds = raw_args
            .get(pos + 1)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(5);
        let cfg = config::load();
        let mut vad = vad::Vad::load(cfg.vad_path())?;
        let (tx, rx) = std::sync::mpsc::channel::<Vec<f32>>();
        let _mic = audio::MicInput::open(tx)?;
        let mut best = 0.0f32;
        let mut peak_rms = 0.0f32;
        let mut frames = 0usize;
        let mut non_silent = 0usize;
        let dump_path = std::env::var("NEUROPIPE_DUMP_MIC").ok();
        let mut dump: Vec<f32> = Vec::new();
        // Turn-end score tracking, mirroring the service loop.
        let mut endpoint = engines::endpoint::HeuristicTurnEnd::new();
        let mut tail: VecDeque<f32> = VecDeque::new();
        let mut silence_ms = 0u64;
        let mut last_scored_ms = 0u64;
        const CHUNK_MS: u64 = 512 * 1000 / 16_000;
        const TAIL_SAMPLES: usize = 16_000 * 2;
        let start = std::time::Instant::now();
        while start.elapsed().as_secs() < seconds {
            match rx.try_recv() {
                Ok(chunk) => {
                    frames += 1;
                    if dump_path.is_some() {
                        dump.extend_from_slice(&chunk);
                    }
                    let rms = (chunk.iter().map(|v| v * v).sum::<f32>() / chunk.len() as f32).sqrt();
                    peak_rms = peak_rms.max(rms);
                    if rms > 0.005 {
                        non_silent += 1;
                    }
                    let p = vad.predict(&chunk).unwrap_or(0.0);
                    best = best.max(p);
                    for &s in &chunk {
                        tail.push_back(s);
                    }
                    while tail.len() > TAIL_SAMPLES {
                        tail.pop_front();
                    }
                    if p > cfg.stt.vad_threshold {
                        silence_ms = 0;
                        last_scored_ms = 0;
                    } else {
                        silence_ms += CHUNK_MS;
                        if silence_ms >= cfg.stt.turn_hold_ms
                            && silence_ms - last_scored_ms >= cfg.stt.turn_score_cadence_ms
                        {
                            let ctx = engines::endpoint::TurnContext {
                                tail: tail.iter().copied().collect(),
                                recording: tail.iter().copied().collect(),
                                silence_ms,
                                utterance_ms: 0,
                                last_vad: p,
                            };
                            let score = endpoint.score(&ctx);
                            last_scored_ms = silence_ms;
                            eprintln!("[dbg] turn score={score:.3} silence={silence_ms}ms");
                        }
                    }
                    if rms > 0.005 {
                        eprintln!("[dbg] frame={frames} len={} rms={rms:.4} vad={p:.4}", chunk.len());
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => std::thread::sleep(std::time::Duration::from_millis(10)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        if let Some(path) = dump_path {
            let raw = dump
                .iter()
                .flat_map(|&v| {
                    let s = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
                    s.to_le_bytes()
                })
                .collect::<Vec<u8>>();
            let _ = std::fs::write(&path, &raw);
            eprintln!("[dbg] dumped {} samples to {}", dump.len(), path);
        }
        println!(
            "frames={frames} peak_rms={peak_rms:.4} non_silent={non_silent} best_vad={best:.4}"
        );
        return Ok(());
    }

    if let Some(pos) = raw_args.iter().position(|a| a == "--smart-turn-wav") {
        let wav = raw_args.get(pos + 1).ok_or_else(|| anyhow::anyhow!("--smart-turn-wav needs a wav path"))?;
        let cfg = config::load();
        let mut engine = engines::smart_turn::SmartTurnEngine::new(cfg.smart_turn_model_path())?;
        let samples = read_wav_f32(wav)?;
        let prob = engine.predict(&samples)?;
        println!("{prob:.6}");
        return Ok(());
    }

    let cfg = config::load();
    let mut svc = service::SttService::new(cfg);
    svc.run()
}

/// Read a RIFF/WAVE file into mono float32 samples, resampling from the file's
/// sample rate (assumed 16000) if needed. Only PCM_16 is supported.
fn read_wav_f32(path: impl AsRef<Path>) -> Result<Vec<f32>> {
    let data = std::fs::read(&path).with_context(|| format!("read {}", path.as_ref().display()))?;
    if &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        bail!("not a RIFF/WAVE file");
    }
    let mut offset = 12;
    let mut audio_fmt: Option<(u16, u32, u16, u16)> = None; // tag, rate, bits, channels
    let mut pcm: Vec<i16> = Vec::new();
    while offset + 8 <= data.len() {
        let chunk_id = &data[offset..offset + 4];
        let size = u32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let body = offset + 8;
        if body + size > data.len() {
            break;
        }
        match chunk_id {
            b"fmt " => {
                let tag = u16::from_le_bytes(data[body..body + 2].try_into().unwrap());
                let rate = u32::from_le_bytes(data[body + 4..body + 8].try_into().unwrap());
                let channels = u16::from_le_bytes(data[body + 2..body + 4].try_into().unwrap());
                let bits = u16::from_le_bytes(data[body + 14..body + 16].try_into().unwrap());
                audio_fmt = Some((tag, rate, bits, channels));
            }
            b"data" => {
                if pcm.is_empty() {
                    let aligned = size;
                    for i in (0..aligned).step_by(2) {
                        if i + 2 <= aligned {
                            let v = i16::from_le_bytes(data[body + i..body + i + 2].try_into().unwrap());
                            pcm.push(v);
                        }
                    }
                }
            }
            _ => {}
        }
        offset = body + size + (size & 1); // chunks are word-aligned
    }

    let (tag, rate, bits, channels) =
        audio_fmt.ok_or_else(|| anyhow::anyhow!("no fmt chunk"))?;
    if tag != 1 || bits != 16 {
        bail!("only PCM_16 supported (tag={tag}, bits={bits})");
    }
    if channels == 0 {
        bail!("no channels");
    }

    let mut samples: Vec<f32> = Vec::with_capacity(pcm.len());
    // Average channels if stereo, otherwise take monophonic.
    for i in (0..pcm.len()).step_by(channels as usize) {
        let mut sum = 0i32;
        let mut cnt = 0u16;
        for c in 0..channels as usize {
            if i + c < pcm.len() {
                sum += pcm[i + c] as i32;
                cnt += 1;
            }
        }
        if cnt > 0 {
            samples.push((sum / cnt as i32) as f32 / 32768.0);
        }
    }

    // Resample 16000 -> 16000 is identity; add nearest-neighbour for others if needed.
    if rate != 16000 {
        let scale = rate as f64 / 16000.0;
        let out_len = (samples.len() as f64 / scale) as usize;
        let mut resampled = Vec::with_capacity(out_len);
        for i in 0..out_len {
            let src = (i as f64 * scale) as usize;
            resampled.push(samples.get(src).copied().unwrap_or(0.0));
        }
        samples = resampled;
    }
    Ok(samples)
}
