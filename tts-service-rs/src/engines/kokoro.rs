use super::{Quality, TtsEngine};
use anyhow::{anyhow, Context, Result};
use ndarray::{s, Array3};
use ndarray_npy::NpzReader;
use ort::session::Session;
use ort::value::Tensor;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

const SAMPLE_RATE: u32 = 24_000;
const MAX_PHONEME_LENGTH: usize = 510;

// This is the Kokoro v1 vocabulary. It is intentionally kept in source so
// the binary does not depend on the Python kokoro_onnx package at runtime.
const VOCAB_JSON: &str = r#"{";":1,":":2,",":3,".":4,"!":5,"?":6,"—":9,"…":10,"\"":11,"(":12,")":13,"“":14,"”":15," ":16,"̃":17,"ʣ":18,"ʥ":19,"ʦ":20,"ʨ":21,"ᵝ":22,"ꭧ":23,"A":24,"I":25,"O":31,"Q":33,"S":35,"T":36,"W":39,"Y":41,"ᵊ":42,"a":43,"b":44,"c":45,"d":46,"e":47,"f":48,"h":50,"i":51,"j":52,"k":53,"l":54,"m":55,"n":56,"o":57,"p":58,"q":59,"r":60,"s":61,"t":62,"u":63,"v":64,"w":65,"x":66,"y":67,"z":68,"ɑ":69,"ɐ":70,"ɒ":71,"æ":72,"β":75,"ɔ":76,"ɕ":77,"ç":78,"ɗ":80,"ð":81,"ʤ":82,"ə":83,"ɚ":85,"ɛ":86,"ɜ":87,"ɟ":90,"ɡ":92,"ɥ":99,"ɨ":101,"ɪ":102,"ʝ":103,"ɯ":110,"ɰ":111,"ŋ":112,"ɳ":113,"ɲ":114,"ɴ":115,"ø":116,"ɸ":118,"θ":119,"œ":120,"ɹ":123,"ɾ":125,"ɻ":126,"ʀ":128,"ɽ":129,"ʂ":130,"ʃ":131,"ʈ":132,"ʧ":133,"ʊ":135,"ʌ":138,"ɣ":139,"ɤ":140,"χ":142,"ʎ":143,"ʒ":147,"ʔ":148,"ˈ":156,"ˌ":157,"ː":158,"ʰ":162,"ʲ":164,"↓":169,"→":171,"↗":172,"↘":173,"ᵻ":177}"#;

impl Quality {
    fn model_name(self) -> &'static str {
        match self {
            Self::Low => "kokoro-v1.0.fp16.onnx",
            Self::High => "kokoro-v1.0.onnx",
        }
    }
}

pub struct Kokoro {
    session: Session,
    voices: NpzReader<BufReader<File>>,
    vocab: HashMap<String, i64>,
    phonemizer: espeak_ng::EspeakNg,
}

pub struct KokoroEngine {
    model_dir: std::path::PathBuf,
    quality: Quality,
    inner: Option<Kokoro>,
}

impl KokoroEngine {
    pub fn new(model_dir: impl AsRef<Path>, quality: Quality) -> Self {
        Self {
            model_dir: model_dir.as_ref().to_path_buf(),
            quality,
            inner: None,
        }
    }
}

impl TtsEngine for KokoroEngine {
    fn load(&mut self) -> Result<()> {
        if self.inner.is_none() {
            self.inner = Some(Kokoro::load(&self.model_dir, self.quality)?);
        }
        Ok(())
    }

    fn unload(&mut self) {
        self.inner = None;
    }

    fn set_quality(&mut self, quality: Quality) -> Result<()> {
        if self.quality != quality {
            self.quality = quality;
            self.unload();
        }
        Ok(())
    }

    fn voices(&mut self) -> Result<Vec<String>> {
        if let Some(inner) = self.inner.as_mut() {
            inner.voices()
        } else {
            Kokoro::list_voice_names(&self.model_dir)
        }
    }

    fn synthesize(&mut self, text: &str, voice: &str, speed: f32) -> Result<(Vec<f32>, u32)> {
        self.load()?;
        self.inner.as_mut().unwrap().synthesize(text, voice, speed)
    }
}

impl Kokoro {
    pub fn list_voice_names(model_dir: impl AsRef<Path>) -> Result<Vec<String>> {
        let voices_path = model_dir.as_ref().join("voices-v1.0.bin");
        let mut voices = NpzReader::new(BufReader::new(File::open(voices_path)?))?
            .names()?
            .into_iter()
            .map(|name| name.trim_end_matches(".npy").to_string())
            .collect::<Vec<_>>();
        voices.sort();
        Ok(voices)
    }

    pub fn load(model_dir: impl AsRef<Path>, quality: Quality) -> Result<Self> {
        let model_dir = model_dir.as_ref();
        let model_path = model_dir.join(quality.model_name());
        let voices_path = model_dir.join("voices-v1.0.bin");
        if !model_path.exists() {
            return Err(anyhow!("Kokoro model not found: {}", model_path.display()));
        }
        if !voices_path.exists() {
            return Err(anyhow!(
                "Kokoro voices not found: {}",
                voices_path.display()
            ));
        }

        let session = Session::builder()
            .map_err(|error| anyhow!("create ONNX session builder: {error}"))?
            .with_intra_threads(4)
            .map_err(|error| anyhow!("configure ONNX session: {error}"))?
            .with_memory_pattern(false)
            .map_err(|error| anyhow!("disable ONNX memory patterns: {error}"))?
            .with_config_entry("CPUExecutionProvider.use_arena", "0")
            .map_err(|error| anyhow!("disable ONNX CPU arena: {error}"))?
            .commit_from_file(&model_path)
            .with_context(|| format!("load Kokoro model {}", model_path.display()))?;
        let voices = NpzReader::new(BufReader::new(File::open(&voices_path)?))?;
        let vocab = serde_json::from_str(VOCAB_JSON)?;
        let phonemizer = espeak_ng::EspeakNg::new("en-us")?;

        Ok(Self {
            session,
            voices,
            vocab,
            phonemizer,
        })
    }

    pub fn voices(&mut self) -> Result<Vec<String>> {
        Ok(self
            .voices
            .names()?
            .into_iter()
            .map(|name| name.trim_end_matches(".npy").to_string())
            .collect())
    }

    pub fn synthesize(&mut self, text: &str, voice: &str, speed: f32) -> Result<(Vec<f32>, u32)> {
        if !(0.5..=2.0).contains(&speed) {
            return Err(anyhow!("speed must be between 0.5 and 2.0"));
        }
        let voice_array: Array3<f32> = self
            .voices
            .by_name(voice)
            .with_context(|| format!("voice '{voice}' not found"))?;
        let phonemes = normalize_phonemes(&self.phonemizer.text_to_phonemes_phonemizer(text)?);
        let filtered: String = phonemes
            .chars()
            .filter(|ch| self.vocab.contains_key(&ch.to_string()))
            .collect();
        let mut audio = Vec::new();

        for batch in split_phonemes(&filtered) {
            let mut token_ids = Vec::with_capacity(batch.chars().count() + 2);
            token_ids.push(0);
            token_ids.extend(
                batch
                    .chars()
                    .filter_map(|ch| self.vocab.get(&ch.to_string()).copied()),
            );
            token_ids.push(0);
            if token_ids.len() > MAX_PHONEME_LENGTH {
                return Err(anyhow!("Kokoro token sequence exceeds 510 tokens"));
            }

            let style_index = token_ids.len() - 2;
            let style: Vec<f32> = voice_array
                .slice(s![style_index, .., ..])
                .iter()
                .copied()
                .collect();
            let tokens = Tensor::from_array((vec![1usize, token_ids.len()], token_ids))?;
            let style = Tensor::from_array(([1usize, 256usize], style))?;
            let speed = Tensor::from_array(([1usize], vec![speed]))?;
            let outputs = self.session.run(ort::inputs![
                "tokens" => tokens,
                "style" => style,
                "speed" => speed,
            ])?;
            let (_, samples) = outputs[0].try_extract_tensor::<f32>()?;
            audio.extend_from_slice(trim_silence(samples));
        }

        Ok((audio, SAMPLE_RATE))
    }
}

fn split_phonemes(phonemes: &str) -> Vec<String> {
    if phonemes.chars().count() <= MAX_PHONEME_LENGTH {
        return vec![phonemes.to_string()];
    }

    let mut parts = Vec::new();
    let mut current = String::new();
    for character in phonemes.chars() {
        current.push(character);
        if matches!(character, '.' | ',' | '!' | '?' | ';') {
            parts.push(current.trim().to_string());
            current.clear();
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }

    let mut batches = Vec::new();
    let mut batch = String::new();
    for part in parts {
        if !batch.is_empty()
            && batch.chars().count() + part.chars().count() + 1 >= MAX_PHONEME_LENGTH
        {
            batches.push(batch.trim().to_string());
            batch.clear();
        }
        if !batch.is_empty() {
            batch.push(' ');
        }
        batch.push_str(&part);
    }
    if !batch.trim().is_empty() {
        batches.push(batch.trim().to_string());
    }
    batches
}

fn normalize_phonemes(phonemes: &str) -> String {
    // The pure-Rust espeak data uses the British diphthong spelling here,
    // while kokoro_onnx's en-us backend emits the American spelling.
    phonemes.replace("əʊ", "oʊ")
}

fn trim_silence(samples: &[f32]) -> &[f32] {
    const FRAME_LENGTH: usize = 2048;
    const HOP_LENGTH: usize = 512;
    if samples.is_empty() {
        return samples;
    }
    // librosa centers its 2048-sample windows with 1024 samples of padding.
    let mut padded = vec![0.0f32; FRAME_LENGTH / 2];
    padded.extend_from_slice(samples);
    padded.resize(padded.len() + FRAME_LENGTH / 2, 0.0);
    let mut frame_rms = Vec::new();
    for start in (0..=padded.len() - FRAME_LENGTH).step_by(HOP_LENGTH) {
        frame_rms.push(
            (padded[start..start + FRAME_LENGTH]
                .iter()
                .map(|sample| sample * sample)
                .sum::<f32>()
                / FRAME_LENGTH as f32)
                .sqrt(),
        );
    }
    let threshold = frame_rms.iter().copied().fold(0.0f32, f32::max) * 0.001;
    if threshold <= 0.0 {
        return samples;
    }
    let mut first = None;
    let mut last = None;
    for (index, rms) in frame_rms.into_iter().enumerate() {
        if rms > threshold {
            first.get_or_insert(index);
            last = Some(index);
        }
    }
    let (Some(first), Some(last)) = (first, last) else {
        return &samples[0..0];
    };
    let start = (first * HOP_LENGTH).min(samples.len());
    let end = ((last + 1) * HOP_LENGTH).min(samples.len());
    &samples[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vocab_has_kokoro_special_tokens() {
        let vocab: HashMap<String, i64> = serde_json::from_str(VOCAB_JSON).unwrap();
        assert_eq!(vocab.get(" "), Some(&16));
        assert_eq!(vocab.get("ˈ"), Some(&156));
    }

    #[test]
    fn split_keeps_short_text_intact() {
        assert_eq!(split_phonemes("hello"), vec!["hello"]);
    }

    #[test]
    fn normalizes_us_diphthong() {
        assert_eq!(normalize_phonemes("həlˈəʊ"), "həlˈoʊ");
    }

    #[test]
    fn trims_silence() {
        let samples = vec![0.0; 3000]
            .into_iter()
            .chain(vec![0.5; 3000])
            .chain(vec![0.0; 3000])
            .collect::<Vec<_>>();
        assert!(trim_silence(&samples).len() < samples.len());
    }
}
