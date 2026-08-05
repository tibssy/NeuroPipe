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
        let config = device
            .default_input_config()
            .context("query default input config")?;

        let channels = config.channels() as usize;
        let device_rate = config.sample_rate().0 as f64;
        let out_rate = SAMPLE_RATE as f64;

        let stream = device
            .build_input_stream(
                &config.config(),
                move |data: &[f32], _| {
                    let mut acc = vec![0.0f32; WINDOW_SIZE];
                    let mut fill = 0usize;
                    for frame in data.chunks_exact(channels) {
                        let mono: f32 = frame.iter().sum::<f32>() / channels as f32;
                        acc[fill] = mono;
                        fill += 1;
                        if fill == WINDOW_SIZE {
                            let frame = if device_rate != out_rate {
                                resample(&acc, device_rate, out_rate)
                            } else {
                                acc.clone()
                            };
                            let _ = tx.send(frame);
                            fill = 0;
                        }
                    }
                },
                move |err| {
                    eprintln!("[STT] mic stream error: {err}");
                },
                None,
            )
            .context("open microphone input stream")?;

        stream.play().context("start microphone stream")?;
        Ok(MicInput { stream })
    }
}

/// Linear resample (good enough for mic capture).
fn resample(input: &[f32], from_rate: f64, to_rate: f64) -> Vec<f32> {
    let scale = from_rate / to_rate;
    let out_len = (input.len() as f64 / scale).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = (i as f64 * scale) as usize;
        out.push(input.get(src).copied().unwrap_or(0.0));
    }
    out
}