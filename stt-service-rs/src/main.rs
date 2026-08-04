mod config;
mod engines;

use anyhow::{bail, Context, Result};
use engines::SttEngine;
use std::env;
use std::path::Path;

fn main() -> Result<()> {
    let raw_args = env::args().skip(1).collect::<Vec<_>>();

    if let Some(pos) = raw_args.iter().position(|a| a == "--transcribe") {
        let wav = raw_args.get(pos + 1).ok_or_else(|| anyhow::anyhow!("--transcribe needs a wav path"))?;
        let cfg = config::load();
        let model_dir = cfg.stt_model_dir();
        let mut engine = engines::parakeet::ParakeetEngine::new(&model_dir, cfg.stt.quantization.clone());
        engine.load()?;
        let samples = read_wav_f32(wav)?;
        let text = engine.transcribe(&samples)?;
        println!("{text}");
        return Ok(());
    }

    let cfg = config::load();
    let model_dir = cfg.stt_model_dir();
    let engine = engines::parakeet::ParakeetEngine::new(&model_dir, cfg.stt.quantization.clone());

    eprintln!("NeuroPipe STT service (native)");
    eprintln!("STT engine: {} (quantization: {})", cfg.stt.model, cfg.stt.quantization);
    eprintln!("STT events: {}", cfg.ipc.stt_pub);
    eprintln!("STT cmd:    {}", cfg.ipc.stt_cmd);

    // Service wiring lands next.
    let _ = engine;
    Ok(())
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