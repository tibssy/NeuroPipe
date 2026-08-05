use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::mpsc;

pub const SAMPLE_RATE: u32 = 16_000;
pub const WINDOW_SIZE: usize = 512;

/// Continuously capture monophonic 16000 Hz audio as fixed 512-sample frames
/// on an mpsc channel. The microcontroller callback never blocks on the bus.
pub struct MicInput {
    #[allow(dead_code)] // kept alive for the stream's RAII lifetime
    stream: cpal::Stream,
}

impl MicInput {
    /// Open the default input device and start streaming `WINDOW_SIZE`-sample
    /// float32 frames to `tx`. Only called once per service run.
    pub fn open(tx: mpsc::Sender<Vec<f32>>) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow!("no default input device"))?;

        // Preferred: request mono 16 kHz F32 natively so the device (via
        // ALSA/PipeWire) resamples once; avoids a second resample in-process.
        let preferred = cpal::StreamConfig {
            channels: 1,
            sample_rate: cpal::SampleRate(SAMPLE_RATE),
            buffer_size: cpal::BufferSize::Default,
        };
        let stream = match device.build_input_stream(
            &preferred,
            {
                let tx = tx.clone();
                move |data: &[f32], _| {
                    for chunk in data.chunks_exact(WINDOW_SIZE) {
                        let _ = tx.send(chunk.to_vec());
                    }
                }
            },
            on_mic_error,
            None,
        ) {
            Ok(stream) => stream,
            Err(_) => {
                // Fall back: open the device's default config, downmix channels
                // to mono and resample to 16 kHz in WINDOW_SIZE frames.
                let config = device
                    .default_input_config()
                    .context("query default input config")?;
                let channels = config.channels() as usize;
                let device_rate = config.sample_rate().0 as f64;
                let out_rate = SAMPLE_RATE as f64;
                let block_len =
                    ((WINDOW_SIZE as f64 * device_rate / out_rate).ceil()) as usize;
                let mut acc: Vec<f32> = Vec::with_capacity(block_len);
                device
                    .build_input_stream(
                        &config.config(),
                        move |data: &[f32], _| {
                            for frame in data.chunks_exact(channels) {
                                let mono: f32 = frame.iter().sum::<f32>() / channels as f32;
                                acc.push(mono);
                                if acc.len() >= block_len {
                                    let frame = resample(&acc, device_rate, out_rate);
                                    acc.clear();
                                    let _ = tx.send(frame);
                                }
                            }
                        },
                        on_mic_error,
                        None,
                    )
                    .context("open microphone input stream (fallback config)")?
            }
        };

        stream.play().context("start microphone stream")?;
        Ok(MicInput { stream })
    }
}

fn on_mic_error(err: cpal::StreamError) {
    eprintln!("[STT] mic stream error: {err}");
}

/// Linear-interpolation resample a device-rate block down to `WINDOW_SIZE`
/// samples at the target rate, so every emitted frame is a full VAD window.
fn resample(input: &[f32], from_rate: f64, to_rate: f64) -> Vec<f32> {
    let scale = from_rate / to_rate;
    let mut out = Vec::with_capacity(WINDOW_SIZE);
    for i in 0..WINDOW_SIZE {
        let pos = i as f64 * scale;
        let a = pos as usize;
        let frac = pos - a as f64;
        let v0 = input.get(a).copied().unwrap_or(0.0);
        let v1 = input.get(a + 1).copied().unwrap_or(v0);
        out.push(v0 + (v1 - v0) * frac as f32);
    }
    out
}

/// Test-only source: replays a 16 kHz PCM16 WAV into the audio channel,
/// then continues emitting silence so the service keeps running.
pub struct FakeMic;

impl FakeMic {
    pub fn open(wav: impl AsRef<std::path::Path>, tx: mpsc::Sender<Vec<f32>>) -> Result<Self> {
        let data = std::fs::read(wav.as_ref())
            .with_context(|| format!("read fake mic wav {}", wav.as_ref().display()))?;
        let mut pcm: Vec<i16> = Vec::new();
        let mut offset = 12;
        while offset + 8 <= data.len() {
            let size =
                u32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap()) as usize;
            if &data[offset..offset + 4] == b"data" {
                for i in (0..size).step_by(2) {
                    if offset + 8 + i + 2 <= data.len() {
                        pcm.push(i16::from_le_bytes(
                            data[offset + 8 + i..offset + 10 + i].try_into().unwrap(),
                        ));
                    }
                }
                break;
            }
            offset = offset + 8 + size + (size & 1);
        }
        let samples: Vec<f32> = pcm.iter().map(|&s| s as f32 / 32768.0).collect();

        std::thread::spawn(move || {
            for frame in samples.chunks(WINDOW_SIZE) {
                let mut buf = vec![0.0f32; WINDOW_SIZE];
                buf[..frame.len()].copy_from_slice(frame);
                if tx.send(buf).is_err() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            // keep sending silence forever so the loop keeps consuming
            loop {
                if tx.send(vec![0.0f32; WINDOW_SIZE]).is_err() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });
        Ok(FakeMic)
    }
}