use anyhow::Result;
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;

/// Silero VAD backed by the standard `silero_vad.onnx` (2.3 MB) model.
///
/// Inputs: `input` [1, 1, T] f32, `sr` scalar i64, `state` [2, 1, 128] f32.
/// Outputs: `output` [1] f32 (speech probability), `stateN` [2, 1, 128].
pub struct Vad {
    session: Session,
    state_h: Vec<f32>,
    state_c: Vec<f32>,
}

impl Vad {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let session = Session::builder()
            .map_err(|e| anyhow::anyhow!("VAD session builder: {e}"))?
            .with_intra_threads(2)
            .map_err(|e| anyhow::anyhow!("VAD threads: {e}"))?
            .with_memory_pattern(false)
            .map_err(|e| anyhow::anyhow!("VAD mem pattern: {e}"))?
            .commit_from_file(path.as_ref())
            .map_err(|e| anyhow::anyhow!("commit VAD {}: {e}", path.as_ref().display()))?;
        Ok(Self { session, state_h: vec![0.0; 128], state_c: vec![0.0; 128] })
    }

    pub fn reset(&mut self) {
        self.state_h.iter_mut().for_each(|v| *v = 0.0);
        self.state_c.iter_mut().for_each(|v| *v = 0.0);
    }

    /// Run VAD over a single 512-sample float32 frame at 16000 Hz.
    pub fn predict(&mut self, frame: &[f32]) -> Result<f32> {
        // input: [1, T]
        let input = Tensor::from_array((vec![1usize, frame.len()], frame.to_vec()))?;
        let sr = Tensor::from_array(([0usize; 0], vec![16000i64]))?;
        // state h and c form [2, 1, 128]
        let mut state = vec![0.0f32; 2 * 128];
        state[0..128].copy_from_slice(&self.state_h);
        state[128..256].copy_from_slice(&self.state_c);
        let state = Tensor::from_array((vec![2usize, 1usize, 128usize], state))?;

        let outputs = self.session.run(ort::inputs!["input" => input, "sr" => sr, "state" => state])?;
        let (_, prob) = outputs["output"].try_extract_tensor::<f32>()?;
        let (_, state_n) = outputs["stateN"].try_extract_tensor::<f32>()?;

        if state_n.len() >= 256 {
            self.state_h.copy_from_slice(&state_n[0..128]);
            self.state_c.copy_from_slice(&state_n[128..256]);
        }

        Ok(prob.first().copied().unwrap_or(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silero_vad_silence_is_low() {
        let path = std::env::var("SILERO_VAD_PATH")
            .unwrap_or_else(|_| "silero_vad.onnx".to_string());
        if !std::path::Path::new(&path).exists() {
            eprintln!("skipping: no silero_vad.onnx at {path}");
            return;
        }
        let mut vad = Vad::load(&path).unwrap();
        let mut prob_sum = 0.0f32;
        for _ in 0..8 {
            prob_sum += vad.predict(&vec![0.0f32; 512]).unwrap();
        }
        assert!(prob_sum / 8.0 < 0.1, "silence should score low, got {prob_sum}");
    }

    #[test]
    fn silero_vad_speech_is_high() {
        let path = std::env::var("SILERO_VAD_PATH").unwrap();
        let wav = std::env::var("PARITY_WAV").unwrap();
        let data = std::fs::read(&wav).unwrap();
        let mut offset = 12;
        let mut pcm: Vec<i16> = Vec::new();
        while offset + 8 <= data.len() {
            let size = u32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap()) as usize;
            if &data[offset..offset + 4] == b"data" {
                for i in (0..size).step_by(2) {
                    pcm.push(i16::from_le_bytes(data[offset + 8 + i..offset + 10 + i].try_into().unwrap()));
                }
                break;
            }
            offset = offset + 8 + size + (size & 1);
        }
        let samples: Vec<f32> = pcm.iter().map(|&s| s as f32 / 32768.0).collect();
        let mut vad = Vad::load(&path).unwrap();
        let mut best = 0.0f32;
        for frame in samples.chunks(512) {
            let mut f = vec![0.0f32; 512];
            f[..frame.len()].copy_from_slice(frame);
            best = best.max(vad.predict(&f).unwrap());
        }
        assert!(best > 0.1, "speech should score above the silence floor, got {best}");
    }
}