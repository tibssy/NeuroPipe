mod config;
mod engines;
mod service;

use anyhow::Result;
use engines::kokoro::Kokoro;
use engines::pocket_tts::PocketTtsEngine;
use engines::supertonic::SupertonicEngine;
use engines::{Quality, TtsEngine};
use std::env;
use std::fs::File;
use std::io::Write;
use std::path::Path;

fn main() -> Result<()> {
    let raw_args = env::args().skip(1).collect::<Vec<_>>();
    if raw_args.is_empty() || raw_args.iter().any(|arg| arg == "--service") {
        return service::TtsService::new(config::load()).run();
    }
    let mut args = raw_args.into_iter();

    let mode = args.next().unwrap_or_else(|| "--kokoro".to_string());
    let text = args.next().unwrap_or_else(|| "Hello world!".to_string());
    let voice = args.next().unwrap_or_else(|| {
        if mode == "--pocket-tts" {
            "alba".to_string()
        } else if mode == "--supertonic-3" {
            "M3".to_string()
        } else {
            "af_heart".to_string()
        }
    });
    let quality = match args.next().as_deref() {
        Some("low") => Quality::Low,
        _ => Quality::High,
    };
    let output = args.next().unwrap_or_else(|| {
        if mode == "--pocket-tts" {
            "pocket-tts-rust.wav".to_string()
        } else if mode == "--supertonic-3" {
            "supertonic-3-rust.wav".to_string()
        } else {
            "kokoro-rust.wav".to_string()
        }
    });

    let (audio, sample_rate) = if mode == "--pocket-tts" {
        let mut engine = PocketTtsEngine::new(
            shellexpand("~/.local/share/neuropipe/models/pocket-tts"),
            quality,
        );
        engine.load()?;
        engine.synthesize(&text, &voice, 1.0)?
    } else if mode == "--supertonic-3" {
        let mut engine = SupertonicEngine::new(
            shellexpand("~/.local/share/neuropipe/models/supertonic-3"),
            quality,
        );
        engine.load()?;
        engine.synthesize(&text, &voice, 1.0)?
    } else {
        let model_dir = shellexpand("~/.local/share/neuropipe/models/kokoro");
        let mut engine = Kokoro::load(&model_dir, quality)?;
        engine.synthesize(&text, &voice, 1.0)?
    };
    write_wav(&output, &audio, sample_rate)?;
    println!(
        "wrote {output} ({} samples at {sample_rate} Hz)",
        audio.len()
    );
    Ok(())
}

fn write_wav(path: impl AsRef<Path>, samples: &[f32], sample_rate: u32) -> Result<()> {
    let mut file = File::create(path)?;
    let pcm: Vec<i16> = samples
        .iter()
        .map(|sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        .collect();
    let data_len = (pcm.len() * 2) as u32;
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + data_len).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&(sample_rate * 2).to_le_bytes())?;
    file.write_all(&2u16.to_le_bytes())?;
    file.write_all(&16u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_len.to_le_bytes())?;
    for sample in pcm {
        file.write_all(&sample.to_le_bytes())?;
    }
    Ok(())
}

fn shellexpand(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_string()
}
