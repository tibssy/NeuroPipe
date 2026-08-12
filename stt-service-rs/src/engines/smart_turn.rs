//! Smart-turn completion classifier (Pipecat smart-turn v3.2, BSD-2-Clause).
//!
//! Scores how likely the current pause marks the end of the user's turn using
//! an ONNX classifier over a Whisper log-mel frontend. The frontend mirrors
//! Pipecat's `WhisperFeatureExtractor(chunk_length=8)` pipeline exactly:
//! right-align the recording to 8 s, zero-mean/unit-variance the waveform over
//! all samples, then compute a Slaney log-mel spectrogram (80 bins, 800
//! frames) clipped to the standard Whisper range. Golden test vectors live in
//! `assets/` and were produced with the upstream Python `inference.py`.

use crate::engines::endpoint::TurnContext;
use crate::engines::TurnEndDetector;
use anyhow::{anyhow, Context, Result};
use ndarray::Array2;
use ort::session::Session;
use ort::value::Tensor;
use realfft::num_complex::Complex;
use realfft::RealFftPlanner;
use std::path::Path;

const SR: usize = 16_000;
const MAX_SAMPLES: usize = 8 * SR; // 128_000
const N_FFT: usize = 400;
const HOP: usize = 160;
const N_MELS: usize = 80;
const N_FRAMES: usize = 800;
const MEL_FLOOR: f64 = 1e-10;
const MIN_LOG_HERTZ: f64 = 1000.0;
const MIN_LOG_MEL: f64 = 15.0;

fn log_step() -> f64 {
    27.0 / 6.4f64.ln()
}

/// Smart-turn turn-end detector backed by the smart-turn-v3.2 ONNX model.
pub struct SmartTurnEngine {
    session: Session,
    min_utterance_ms: u64,
    mel_filters: Array2<f64>,
    window: Vec<f64>,
}

impl SmartTurnEngine {
    /// Load the model and precompute the (fixed) mel filter bank + window.
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(anyhow!("missing smart-turn model: {}", path.display()));
        }
        let session = Session::builder()
            .map_err(|e| anyhow!("smart-turn session builder: {e}"))?
            .with_intra_threads(1)
            .map_err(|e| anyhow!("smart-turn threads: {e}"))?
            .with_memory_pattern(false)
            .map_err(|e| anyhow!("smart-turn mem pattern: {e}"))?
            .with_config_entry("CPUExecutionProvider.use_arena", "0")
            .map_err(|e| anyhow!("smart-turn arena: {e}"))?
            .commit_from_file(path)
            .with_context(|| format!("load smart-turn model {}", path.display()))?;
        let mel_filters = mel_filter_bank();
        let window = hann_window();
        Ok(Self {
            session,
            min_utterance_ms: 1200,
            mel_filters,
            window,
        })
    }

    pub fn with_min_utterance_ms(mut self, ms: u64) -> Self {
        self.min_utterance_ms = ms;
        self
    }

    /// Run the classifier over a recording. Returns P(end-of-turn) in (0,1).
    pub fn predict(&mut self, recording: &[f32]) -> Result<f32> {
        let features = log_mel_features(recording, &self.mel_filters, &self.window);
        let data: Vec<f32> = features.iter().copied().collect();
        let input = Tensor::from_array((vec![1usize, N_MELS, N_FRAMES], data))?;
        let outputs = self.session.run(ort::inputs!["input_features" => input])?;
        let (_, prob) = outputs["logits"].try_extract_tensor::<f32>()?;
        Ok(prob.first().copied().unwrap_or(0.0))
    }
}

impl TurnEndDetector for SmartTurnEngine {
    fn score(&mut self, ctx: &TurnContext) -> f32 {
        if ctx.utterance_ms < self.min_utterance_ms {
            return 0.0;
        }
        self.predict(&ctx.recording).unwrap_or(0.0)
    }

    fn reset(&mut self) {}
}

/// Whisper log-mel frontend: (80, 800) f32 features for a 16 kHz recording.
///
/// Ported from transformers' numpy `_np_extract_fbank_features` and Pipecat's
/// `truncate_audio_to_last_n_seconds` (see golden tests in this module).
pub fn log_mel_features(audio: &[f32], mel_filters: &Array2<f64>, window: &[f64]) -> Array2<f32> {
    // Right-align to 8 s: keep the end, or left-pad with zeros.
    let mut buf = vec![0.0f32; MAX_SAMPLES];
    if audio.len() > MAX_SAMPLES {
        buf.copy_from_slice(&audio[audio.len() - MAX_SAMPLES..]);
    } else {
        buf[MAX_SAMPLES - audio.len()..].copy_from_slice(audio);
    }

    // Zero-mean / unit-variance over all 128_000 samples (population variance).
    // numpy promotes to float64 for the mean, so accumulate in f64.
    let mean = buf.iter().map(|&v| v as f64).sum::<f64>() / buf.len() as f64;
    let mut acc = 0.0f64;
    for &v in &buf {
        let d = v as f64 - mean;
        acc += d * d;
    }
    let var = acc / buf.len() as f64;
    let scale = (var + 1e-7).sqrt();
    for v in buf.iter_mut() {
        *v = ((*v as f64 - mean) / scale) as f32;
    }

    // Reflect-pad by n_fft/2 on each side (numpy np.pad mode="reflect").
    let pad = N_FFT / 2;
    let n = buf.len();
    let mut waveform = Vec::with_capacity(n + 2 * pad);
    for i in 0..pad {
        waveform.push(buf[pad - i]);
    }
    waveform.extend_from_slice(&buf);
    for j in 0..pad {
        waveform.push(buf[n - 2 - j]);
    }

    // Frame the (padded) waveform: power spectrogram [frames, bins].
    let num_frames = 1 + (waveform.len() - N_FFT) / HOP; // 801
    let num_bins = N_FFT / 2 + 1; // 201
    let mut spec = Array2::<f64>::zeros((num_frames, num_bins));

    let mut planner = RealFftPlanner::<f64>::new();
    let r2c = planner.plan_fft_forward(N_FFT);
    let mut windowed = r2c.make_input_vec();
    let mut spectrum: Vec<Complex<f64>> = r2c.make_output_vec();

    for t in 0..num_frames {
        let start = t * HOP;
        for (k, s) in windowed.iter_mut().enumerate() {
            *s = waveform[start + k] as f64 * window[k];
        }
        // realfft output is normalized like numpy rfft (unnormalized forward).
        let _ = r2c.process(&mut windowed, &mut spectrum);
        for (b, c) in spectrum.iter().enumerate() {
            spec[[t, b]] = c.re * c.re + c.im * c.im;
        }
    }

    // Mel filter bank: filters.T @ spectrogram, floor, log10, drop last frame.
    let mut mel = Array2::<f64>::zeros((N_MELS, num_frames));
    for (m, mut row) in mel.rows_mut().into_iter().enumerate() {
        for (t, cell) in row.iter_mut().enumerate() {
            let mut acc = 0.0f64;
            for b in 0..num_bins {
                acc += mel_filters[[b, m]] * spec[[t, b]];
            }
            *cell = acc.max(MEL_FLOOR).log10();
        }
    }

    // log_spec[:, :-1], clip to max-8, scale (x+4)/4 — all in float32.
    let mut features = Array2::<f32>::zeros((N_MELS, N_FRAMES));
    let mut max = f32::NEG_INFINITY;
    for m in 0..N_MELS {
        for t in 0..N_FRAMES {
            let v = mel[[m, t]] as f32;
            max = max.max(v);
        }
    }
    let floor = max - 8.0;
    for m in 0..N_MELS {
        for t in 0..N_FRAMES {
            features[[m, t]] = (mel[[m, t]] as f32).max(floor);
        }
    }
    features.mapv_inplace(|v| (v + 4.0) / 4.0);
    features
}

/// Periodic Hann window of length 400 (np.hanning(401)[:-1]).
fn hann_window() -> Vec<f64> {
    (0..N_FFT)
        .map(|n| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / N_FFT as f64).cos())
        .collect()
}

fn hertz_to_mel(freq: f64) -> f64 {
    if freq >= MIN_LOG_HERTZ {
        MIN_LOG_MEL + (freq / MIN_LOG_HERTZ).ln() * log_step()
    } else {
        3.0 * freq / 200.0
    }
}

fn mel_to_hertz(mel: f64) -> f64 {
    if mel >= MIN_LOG_MEL {
        MIN_LOG_HERTZ * ((mel - MIN_LOG_MEL) / log_step()).exp()
    } else {
        200.0 * mel / 3.0
    }
}

/// Slaney mel filter bank, 80 filters over 0-8 kHz, 201 FFT bins, with the
/// Slaney area normalization (transformers `mel_filter_bank`).
fn mel_filter_bank() -> Array2<f64> {
    let num_bins = N_FFT / 2 + 1;
    let mel_min = hertz_to_mel(0.0);
    let mel_max = hertz_to_mel(8000.0);
    // 82 evenly spaced mel points -> filter_freqs in Hz.
    let mut filter_freqs = Vec::with_capacity(N_MELS + 2);
    for i in 0..N_MELS + 2 {
        let mel = mel_min + (mel_max - mel_min) * (i as f64 / (N_MELS + 1) as f64);
        filter_freqs.push(mel_to_hertz(mel));
    }
    // FFT bin frequencies, 0..=8000 Hz in 201 steps.
    let fft_freqs: Vec<f64> = (0..num_bins)
        .map(|i| i as f64 * 8000.0 / (num_bins - 1) as f64)
        .collect();

    let mut filters = Array2::<f64>::zeros((num_bins, N_MELS));
    for m in 0..N_MELS {
        let left = filter_freqs[m];
        let center = filter_freqs[m + 1];
        let right = filter_freqs[m + 2];
        for (b, &f) in fft_freqs.iter().enumerate() {
            let rising = (f - left) / (center - left);
            let falling = (right - f) / (right - center);
            filters[[b, m]] = (rising.min(falling)).max(0.0);
        }
        let enorm = 2.0 / (right - left);
        for b in 0..num_bins {
            filters[[b, m]] *= enorm;
        }
    }
    filters
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_wav_f32(path: &str) -> Vec<f32> {
        let data = std::fs::read(path).unwrap();
        let mut offset = 12;
        let mut pcm: Vec<i16> = Vec::new();
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
        pcm.iter().map(|&s| s as f32 / 32768.0).collect()
    }

    fn asset_dir() -> String {
        format!("{}/assets", env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn frontend_matches_python_reference_mel() {
        let filters = mel_filter_bank();
        let window = hann_window();
        for name in ["tone", "speech"] {
            let wav = format!("{}/smart_turn_fixture_{name}.wav", asset_dir());
            let audio = read_wav_f32(&wav);
            let mel = log_mel_features(&audio, &filters, &window);
            let reference_path = format!("{}/smart_turn_fixture_{name}_mel.bin", asset_dir());
            let bytes = std::fs::read(&reference_path).unwrap();
            let expected_len = N_MELS * N_FRAMES * 4;
            assert_eq!(bytes.len(), expected_len, "{name}: reference size");
            let mut max_diff = 0.0f32;
            for (i, chunk) in bytes.chunks_exact(4).enumerate() {
                let expected = f32::from_le_bytes(chunk.try_into().unwrap());
                let actual = mel[[i / N_FRAMES, i % N_FRAMES]];
                max_diff = max_diff.max((actual - expected).abs());
            }
            assert!(
                max_diff < 1e-4,
                "{name}: rust frontend diverges from python reference, max diff {max_diff}"
            );
        }
    }

    #[test]
    fn frontend_produces_expected_probabilities() {
        let model_path = std::env::var("SMART_TURN_MODEL_PATH")
            .unwrap_or_else(|_| "smart_turn_v3.2_cpu.onnx".to_string());
        if !Path::new(&model_path).exists() {
            eprintln!("skipping: no smart-turn model at {model_path}");
            return;
        }
        let mut engine = SmartTurnEngine::new(&model_path).unwrap();
        let reference = serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(format!("{}/smart_turn_reference.json", asset_dir())).unwrap(),
        )
        .unwrap();
        for name in ["tone", "speech"] {
            let wav = format!("{}/smart_turn_fixture_{name}.wav", asset_dir());
            let audio = read_wav_f32(&wav);
            let prob = engine.predict(&audio).unwrap();
            let expected = reference[&format!("smart_turn_fixture_{name}.wav")]
                .as_f64()
                .unwrap();
            assert!(
                (prob as f64 - expected).abs() < 1e-4,
                "{name}: prob {prob} != python reference {expected}"
            );
        }
    }

    #[test]
    fn mel_filter_bank_shapes_and_finite() {
        let filters = mel_filter_bank();
        assert_eq!(filters.shape(), &[N_FFT / 2 + 1, N_MELS]);
        assert!(filters.iter().all(|x| x.is_finite()));
    }
}
