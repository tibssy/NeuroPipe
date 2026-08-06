use crate::engines::SttEngine;
use anyhow::{anyhow, Context, Result};
use ndarray::Array2;
use ort::session::Session;
use ort::value::Tensor;
use realfft::num_complex::Complex;
use realfft::RealFftPlanner;
use std::path::Path;

// NeMo conformer log-mel preprocessor parameters (see onnx-asr NemoPreprocessor).
const N_FFT: usize = 512;
const WIN_LENGTH: usize = 400;
const HOP_LENGTH: usize = 160;
const LOG_ZERO_GUARD: f32 = 1.0 / 16_777_216.0; // 2^-24
const PREEMPH: f32 = 0.97;

const NEMO80: &[u8] = include_bytes!("../../assets/nemo80_fbanks.bin");
const NEMO128: &[u8] = include_bytes!("../../assets/nemo128_fbanks.bin");

/// Self-contained Parakeet-TDT engine mirroring onnx-asr's
/// `nemo-conformer-tdt` implementation, but with native ort inference.
pub struct Parakeet {
    encoder: Session,
    decoder_joint: Session,
    vocab: Vec<String>,
    blank_idx: usize,
    vocab_size: usize,
    features_size: usize,
    max_tokens_per_step: usize,
    state1_shape: Vec<i64>,
    state2_shape: Vec<i64>,
    fbanks: Array2<f32>,
    window: Vec<f32>,
}

pub struct ParakeetEngine {
    model_dir: std::path::PathBuf,
    inner: Option<Parakeet>,
}

impl ParakeetEngine {
    pub fn new(model_dir: impl AsRef<Path>) -> Self {
        Self {
            model_dir: model_dir.as_ref().to_path_buf(),
            inner: None,
        }
    }
}

impl SttEngine for ParakeetEngine {
    fn load(&mut self) -> Result<()> {
        if self.inner.is_none() {
            self.inner = Some(Parakeet::load(&self.model_dir)?);
        }
        Ok(())
    }

    fn unload(&mut self) {
        self.inner = None;
    }

    fn is_loaded(&self) -> bool {
        self.inner.is_some()
    }

    fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        self.load()?;
        self.inner.as_mut().unwrap().transcribe(samples)
    }
}

impl Parakeet {
    fn load(model_dir: &Path) -> Result<Self> {
        let encoder_path = model_dir.join("encoder-model.int8.onnx");
        let decoder_path = model_dir.join("decoder_joint-model.int8.onnx");
        let vocab_path = model_dir.join("vocab.txt");
        let config_path = model_dir.join("config.json");

        for (name, path) in [
            ("encoder", encoder_path.as_path()),
            ("decoder_joint", decoder_path.as_path()),
            ("vocab", vocab_path.as_path()),
        ] {
            if !path.exists() {
                return Err(anyhow!("missing {name} model file: {}", path.display()));
            }
        }

        let encoder = build_session(&encoder_path)
            .with_context(|| format!("load encoder {}", encoder_path.display()))?;
        let decoder_joint = build_session(&decoder_path)
            .with_context(|| format!("load decoder_joint {}", decoder_path.display()))?;

        let vocab = load_vocab(&vocab_path)?;
        let blank_idx = vocab
            .iter()
            .position(|t| t == "<blk>")
            .ok_or_else(|| anyhow!("vocab has no <blk> token"))?;
        let vocab_size = vocab.len();

        // Model config (features / max tokens per step).
        let (features_size, max_tokens_per_step) =
            if config_path.exists() {
                let raw = std::fs::read_to_string(&config_path)?;
                let cfg: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
                let gi = |key: &str, default: usize| {
                    cfg.get(key).and_then(|v| v.as_u64()).map(|x| x as usize).unwrap_or(default)
                };
                (gi("features_size", 80), gi("max_tokens_per_step", 10))
            } else {
                (80, 10)
            };

        if !matches!(features_size, 80 | 128) {
            return Err(anyhow!("unsupported features_size: {features_size}"));
        }

        // Allocate zero LSTM decoder state from the joint model's declared input shapes.
        let input_shapes = decoder_joint
            .inputs()
            .iter()
            .map(|i| (i.name().to_string(), i.dtype()))
            .collect::<Vec<_>>();
        let state1_shape = state_shape(&input_shapes, "input_states_1")?;
        let state2_shape = state_shape(&input_shapes, "input_states_2")?;

        let fbanks = load_fbanks(if features_size == 80 { NEMO80 } else { NEMO128 }, features_size)?;
        let window = build_window();

        Ok(Self {
            encoder,
            decoder_joint,
            vocab,
            blank_idx,
            vocab_size,
            features_size,
            max_tokens_per_step,
            state1_shape,
            state2_shape,
            fbanks,
            window,
        })
    }

    fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        let (features, features_len) = self.preprocess(samples)?;
        let encoder_out = self.encode(&features, features_len)?;
        let tokens = self.decode_greedy(&encoder_out)?;
        Ok(decode_text(&tokens, &self.vocab))
    }

    /// Log-mel spectrogram + instance normalisation (NemoPreprocessor).
    fn preprocess(&self, samples: &[f32]) -> Result<(Array2<f32>, usize)> {
        let n = samples.len();
        let features_len = n / HOP_LENGTH;

        // Pre-emphasis: y[0]=x[0], y[i]=x[i]-0.97*x[i-1] (Python zeroes samples >= n, a no-op here).
        let mut emph = vec![0.0f32; n];
        if n > 0 {
            emph[0] = samples[0];
            for i in 1..n {
                emph[i] = samples[i] - PREEMPH * samples[i - 1];
            }
        }

        // Zero-pad n_fft/2 on both sides.
        let pad = N_FFT / 2;
        let total = n + 2 * pad;
        let mut buf = vec![0.0f32; total];
        buf[pad..pad + n].copy_from_slice(&emph);

        let n_windows = if n == 0 { 0 } else { (total - N_FFT) / HOP_LENGTH + 1 };

        let nfilt = self.features_size;
        let mut mel = Array2::<f32>::zeros((n_windows, nfilt));

        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(N_FFT);
        let mut windowed = r2c.make_input_vec();
        let mut spectrum: Vec<Complex<f32>> = r2c.make_output_vec();

        for t in 0..n_windows {
            let start = t * HOP_LENGTH;
            for k in 0..N_FFT {
                windowed[k] = buf[start + k] * self.window[k];
            }
            r2c.process(&mut windowed, &mut spectrum).map_err(|e| anyhow!("fft: {e}"))?;
            for j in 0..nfilt {
                let mut acc = 0.0f32;
                for (k, c) in spectrum.iter().enumerate() {
                    let p = c.re * c.re + c.im * c.im;
                    acc += p * self.fbanks[[k, j]];
                }
                mel[[t, j]] = (acc + LOG_ZERO_GUARD).ln();
            }
        }

        // Instance normalisation across the time axis, masked by features_len.
        if features_len > 0 {
            let denom = if features_len > 1 { features_len as f32 - 1.0 } else { 1.0 };
            for j in 0..nfilt {
                let mut mean = 0.0f32;
                for t in 0..features_len {
                    mean += mel[[t, j]];
                }
                mean /= features_len as f32;
                let mut var = 0.0f32;
                for t in 0..features_len {
                    let d = mel[[t, j]] - mean;
                    var += d * d;
                }
                var /= denom;
                let std = var.sqrt() + 1e-5;
                for t in 0..features_len {
                    mel[[t, j]] = (mel[[t, j]] - mean) / std;
                }
                for t in features_len..n_windows {
                    mel[[t, j]] = 0.0;
                }
            }
        }

        // Transpose to [features_size, time].
        let mut features = Array2::<f32>::zeros((nfilt, n_windows));
        for t in 0..n_windows {
            for j in 0..nfilt {
                features[[j, t]] = mel[[t, j]];
            }
        }

        Ok((features, features_len))
    }

    /// Run the Conformer encoder; returns encoder output as [T_enc, 1024].
    fn encode(&mut self, features: &Array2<f32>, features_len: usize) -> Result<ndarray::Array2<f32>> {
        let fs = self.features_size;
        let n_t = features.shape()[1];
        let mut audio_data = vec![0.0f32; 1 * fs * n_t];
        for f in 0..fs {
            for t in 0..n_t {
                audio_data[f * n_t + t] = features[[f, t]];
            }
        }
        let audio_signal = Tensor::from_array((vec![1usize, fs, n_t], audio_data))?;
        let length = Tensor::from_array((vec![1usize], vec![features_len as i64]))?;

        let outputs = self.encoder.run(ort::inputs!["audio_signal" => audio_signal, "length" => length])?;
        let (_, enc_data) = outputs["outputs"].try_extract_tensor::<f32>()?;
        let (_, enc_len_data) = outputs["encoded_lengths"].try_extract_tensor::<i64>()?;

        let enc_len = enc_len_data.first().copied().unwrap_or(0) as usize;
        // encoder output is [1, features, T_enc] (feature-major, time minor); the
        // t-th frame's feature vector is strided, not contiguous. Reshape to
        // [T_enc, features] the way onnx-asr's encoder_out.transpose(0,2,1) does.
        // NOTE: the model may emit one more padded time frame than encoded_lengths
        // (e.g. 81 frames vs enc_len 80), so stride by the actual time extent.
        if enc_data.is_empty() || enc_len == 0 {
            return Ok(ndarray::Array2::<f32>::zeros((0, 1024)));
        }
        let feature_dim = 1024;
        let t_enc = enc_data.len() / feature_dim;
        let frames = enc_len.min(t_enc);
        let mut out = ndarray::Array2::<f32>::zeros((frames, feature_dim));
        for (t, mut row) in out.rows_mut().into_iter().enumerate() {
            for (f, cell) in row.iter_mut().enumerate() {
                *cell = enc_data[f * t_enc + t];
            }
        }
        Ok(out)
    }

    /// Greedy TDT decoding loop (asr.py `_AsrWithTransducerDecoding._decoding`).
    fn decode_greedy(&mut self, encoder_out: &ndarray::Array2<f32>) -> Result<Vec<usize>> {
        let t_enc = encoder_out.shape()[0];
        let (mut s1, mut s2) = self.zero_state()?;
        let mut tokens: Vec<usize> = Vec::new();
        let mut emitted = 0usize;
        let mut t = 0usize;

        while t < t_enc {
            let (logits, step, ns1, ns2) = self.step(&tokens, &s1, &s2, t, encoder_out)?;
            let token = argmax(&logits);

            if token != self.blank_idx {
                s1 = ns1;
                s2 = ns2;
                tokens.push(token);
                emitted += 1;
            }

            if step > 0 {
                t += step;
                emitted = 0;
            } else if token == self.blank_idx || emitted == self.max_tokens_per_step {
                t += 1;
                emitted = 0;
            }
        }
        Ok(tokens)
    }

    /// One TDT decoder_joint step. Returns (token logits, duration argmax, new states).
    fn step(
        &mut self,
        tokens: &[usize],
        s1: &[f32],
        s2: &[f32],
        t: usize,
        encoder_out: &ndarray::Array2<f32>,
    ) -> Result<(Vec<f32>, usize, Vec<f32>, Vec<f32>)> {
        let prev = tokens.last().copied().unwrap_or(self.blank_idx) as i32;

        let frame_len = encoder_out.shape()[1];
        let frame: Vec<f32> = encoder_out.row(t).to_vec();
        let encoder_outputs = Tensor::from_array((vec![1usize, frame_len, 1], frame))?;
        let targets = Tensor::from_array((vec![1usize, 1usize], vec![prev]))?;
        let target_length = Tensor::from_array((vec![1usize], vec![1i32]))?;
        let input_states_1 = Tensor::from_array((self.state1_shape.iter().map(|&d| d as usize).collect::<Vec<_>>(), s1.to_vec()))?;
        let input_states_2 = Tensor::from_array((self.state2_shape.iter().map(|&d| d as usize).collect::<Vec<_>>(), s2.to_vec()))?;

        let outputs = self.decoder_joint.run(ort::inputs![
            "encoder_outputs" => encoder_outputs,
            "targets" => targets,
            "target_length" => target_length,
            "input_states_1" => input_states_1,
            "input_states_2" => input_states_2,
        ])?;

        let (_, logits_slice) = outputs["outputs"].try_extract_tensor::<f32>()?;
        let (_, os1) = outputs["output_states_1"].try_extract_tensor::<f32>()?;
        let (_, os2) = outputs["output_states_2"].try_extract_tensor::<f32>()?;

        // output = [token_logits(0..V) | duration_logits(V..)]
        let token_logits = logits_slice[..self.vocab_size.min(logits_slice.len())].to_vec();
        let duration_logits = &logits_slice[self.vocab_size.min(logits_slice.len())..];
        let step = if duration_logits.is_empty() { 0 } else { argmax(duration_logits) };

        Ok((token_logits, step, os1.to_vec(), os2.to_vec()))
    }

    fn zero_state(&self) -> Result<(Vec<f32>, Vec<f32>)> {
        let n1 = self.state1_shape.iter().map(|&d| d.max(0) as usize).product::<usize>();
        let n2 = self.state2_shape.iter().map(|&d| d.max(0) as usize).product::<usize>();
        Ok((vec![0.0; n1], vec![0.0; n2]))
    }
}

fn build_session(path: &Path) -> Result<Session> {
    Session::builder()
        .map_err(|error| anyhow!("create ONNX session builder: {error}"))?
        .with_intra_threads(4)
        .map_err(|error| anyhow!("configure ONNX session: {error}"))?
        .with_memory_pattern(false)
        .map_err(|error| anyhow!("disable ONNX memory patterns: {error}"))?
        .with_config_entry("CPUExecutionProvider.use_arena", "0")
        .map_err(|error| anyhow!("disable ONNX CPU arena: {error}"))?
        .commit_from_file(path)
        .map_err(|error| anyhow!("ort commit {path:?}: {error}"))
}

fn state_shape(inputs: &[(String, &ort::value::ValueType)], name: &str) -> Result<Vec<i64>> {
    let (_, dtype) = inputs
        .iter()
        .find(|(n, _)| n == name)
        .ok_or_else(|| anyhow!("model input not found: {name}"))?;
    match dtype {
        ort::value::ValueType::Tensor { shape, .. } => Ok(shape
            .as_ref()
            .iter()
            .map(|&d| if d <= 0 { 1 } else { d })
            .collect()),
        other => Err(anyhow!("unexpected type for {name}: {other:?}")),
    }
}

fn load_vocab(path: &Path) -> Result<Vec<String>> {
    let content = std::fs::read_to_string(path)?;
    let mut max_id: usize = 0;
    let mut entries: Vec<(usize, String)> = Vec::new();
    for line in content.lines() {
        let line = line.trim_end_matches('\n');
        let mut parts = line.splitn(2, ' ');
        let (Some(token), Some(id)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(id) = id.trim().parse::<usize>() else {
            continue;
        };
        let token = token.replace('\u{2581}', " ");
        max_id = max_id.max(id);
        entries.push((id, token));
    }
    let mut vocab = vec![String::new(); max_id + 1];
    for (id, token) in entries {
        vocab[id] = token;
    }
    Ok(vocab)
}

fn load_fbanks(bytes: &[u8], nfilt: usize) -> Result<Array2<f32>> {
    if bytes.len() < 12 || &bytes[0..4] != b"NPMB" {
        return Err(anyhow!("bad filterbank asset header"));
    }
    let rows = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let cols = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    if cols != nfilt {
        return Err(anyhow!("filterbank col mismatch: {cols} != {nfilt}"));
    }
    let expected = 12 + rows * cols * 4;
    if bytes.len() != expected {
        return Err(anyhow!("filterbank size mismatch"));
    }
    let data_start = &bytes[12..];
    let mut fb = Array2::<f32>::zeros((rows, cols));
    for idx in 0..(rows * cols) {
        let offset = idx * 4;
        let v = f32::from_le_bytes(
            data_start[offset..offset + 4].try_into().unwrap(),
        );
        fb[(idx / cols, idx % cols)] = v;
    }
    Ok(fb)
}

fn build_window() -> Vec<f32> {
    let mut win = vec![0.0f32; N_FFT];
    let start = (N_FFT - WIN_LENGTH) / 2;
    let period = (WIN_LENGTH - 1) as f32;
    for i in 0..WIN_LENGTH {
        let x = 2.0 * core::f32::consts::PI * (i as f32) / period;
        win[start + i] = 0.5 - 0.5 * x.cos();
    }
    win
}

fn argmax(data: &[f32]) -> usize {
    data.iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Join vocabulary tokens and collapse whitespace (mirrors onnx-asr decode).
fn decode_text(tokens: &[usize], vocab: &[String]) -> String {
    let mut joined = String::new();
    for &id in tokens {
        if let Some(token) = vocab.get(id) {
            joined.push_str(token);
        }
    }
    joined.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filterbank_asset_loads() {
        let fb80 = load_fbanks(NEMO80, 80).unwrap();
        let fb128 = load_fbanks(NEMO128, 128).unwrap();
        assert_eq!(fb80.shape(), &[257, 80]);
        assert_eq!(fb128.shape(), &[257, 128]);
        assert!(fb80.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn decode_collapses_spaces() {
        // load_vocab already replaced sentencepiece ▁ with spaces.
        let vocab: Vec<String> = vec!["<blk>".into(), " Hello".into(), " world".into()];
        assert_eq!(decode_text(&[1, 2], &vocab), "Hello world");
        assert_eq!(decode_text(&[1], &vocab), "Hello");
        assert_eq!(decode_text(&[1, 2, 1, 2], &vocab), "Hello world Hello world");
    }
}
