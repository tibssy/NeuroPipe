use super::{Quality, TtsEngine};
use anyhow::{anyhow, Context, Result};
use ort::session::Session;
use ort::value::Tensor;
use rand::rng;
use rand_distr::{Distribution, Normal};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

const MIN_SPEED: f32 = 0.7;
const MAX_SPEED: f32 = 2.0;
const DEFAULT_MAX_CHUNK_LENGTH: usize = 300;
const DEFAULT_MAX_CHUNK_LENGTH_KO: usize = 120;
const SILENCE_DURATION: f32 = 0.3;

type TensorData = (Vec<usize>, Vec<f32>);

fn steps_for_quality(quality: Quality) -> usize {
    match quality {
        Quality::Low => 4,
        Quality::High => 8,
    }
}

pub struct SupertonicEngine {
    model_dir: PathBuf,
    quality: Quality,
    inner: Option<Supertonic>,
}

impl SupertonicEngine {
    pub fn new(model_dir: impl AsRef<Path>, quality: Quality) -> Self {
        Self {
            model_dir: model_dir.as_ref().to_path_buf(),
            quality,
            inner: None,
        }
    }
}

impl TtsEngine for SupertonicEngine {
    fn load(&mut self) -> Result<()> {
        if self.inner.is_none() {
            self.inner = Some(Supertonic::load(&self.model_dir)?);
        }
        Ok(())
    }

    fn unload(&mut self) {
        self.inner = None;
    }

    fn set_quality(&mut self, quality: Quality) -> Result<()> {
        self.quality = quality;
        Ok(())
    }

    fn voices(&mut self) -> Result<Vec<String>> {
        Supertonic::list_voice_names(&self.model_dir)
    }

    fn synthesize(&mut self, text: &str, voice: &str, speed: f32) -> Result<(Vec<f32>, u32)> {
        self.load()?;
        self.inner
            .as_mut()
            .unwrap()
            .synthesize(text, voice, speed, steps_for_quality(self.quality))
    }
}

struct Supertonic {
    sample_rate: u32,
    base_chunk_size: usize,
    chunk_compress_factor: usize,
    latent_dim: usize,
    indexer: Vec<i32>,
    dp: Session,
    text_enc: Session,
    vector_est: Session,
    vocoder: Session,
    voices_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
struct TtsConfigFile {
    ae: AeConfig,
    ttl: TtlConfig,
}

#[derive(Debug, Deserialize)]
struct AeConfig {
    sample_rate: u32,
    base_chunk_size: usize,
}

#[derive(Debug, Deserialize)]
struct TtlConfig {
    latent_dim: usize,
    chunk_compress_factor: usize,
}

#[derive(Debug, Deserialize)]
struct StyleTensorFile {
    dims: Vec<usize>,
    data: Vec<Vec<Vec<f32>>>,
}

#[derive(Debug, Deserialize)]
struct StyleFile {
    style_ttl: StyleTensorFile,
    style_dp: StyleTensorFile,
}

struct Style {
    style_ttl: TensorData,
    style_dp: TensorData,
}

impl Supertonic {
    fn list_voice_names(model_dir: &Path) -> Result<Vec<String>> {
        let voices_dir = model_dir.join("voice_styles");
        let mut voices = fs::read_dir(&voices_dir)?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
            .filter(|name| name.ends_with(".json"))
            .map(|name| name.trim_end_matches(".json").to_string())
            .collect::<Vec<_>>();
        voices.sort();
        Ok(voices)
    }

    fn load(model_dir: &Path) -> Result<Self> {
        let onnx_dir = model_dir.join("onnx");
        let cfgs: TtsConfigFile = serde_json::from_str(&fs::read_to_string(onnx_dir.join("tts.json"))
            .with_context(|| format!("read {}", onnx_dir.join("tts.json").display()))?)
            .context("parse tts.json")?;
        let indexer: Vec<i32> = serde_json::from_str(
            &fs::read_to_string(onnx_dir.join("unicode_indexer.json"))
                .with_context(|| format!("read {}", onnx_dir.join("unicode_indexer.json").display()))?,
        )
        .context("parse unicode_indexer.json")?;
        let dp = load_session(&onnx_dir, "duration_predictor")?;
        let text_enc = load_session(&onnx_dir, "text_encoder")?;
        let vector_est = load_session(&onnx_dir, "vector_estimator")?;
        let vocoder = load_session(&onnx_dir, "vocoder")?;
        let voices_dir = model_dir.join("voice_styles");
        if !voices_dir.is_dir() {
            return Err(anyhow!(
                "Supertonic voice styles directory not found: {}",
                voices_dir.display()
            ));
        }
        Ok(Self {
            sample_rate: cfgs.ae.sample_rate,
            base_chunk_size: cfgs.ae.base_chunk_size,
            chunk_compress_factor: cfgs.ttl.chunk_compress_factor,
            latent_dim: cfgs.ttl.latent_dim,
            indexer,
            dp,
            text_enc,
            vector_est,
            vocoder,
            voices_dir,
        })
    }

    fn synthesize(
        &mut self,
        text: &str,
        voice: &str,
        speed: f32,
        total_steps: usize,
    ) -> Result<(Vec<f32>, u32)> {
        if !(MIN_SPEED..=MAX_SPEED).contains(&speed) {
            return Err(anyhow!("speed must be between {MIN_SPEED} and {MAX_SPEED}"));
        }
        let lang = detect_lang(text);
        let style = self.load_style(voice)?;
        let max_chunk = if lang == "ko" {
            DEFAULT_MAX_CHUNK_LENGTH_KO
        } else {
            DEFAULT_MAX_CHUNK_LENGTH
        };
        let chunks = chunk_text(text, max_chunk);
        let silence = vec![0.0f32; (SILENCE_DURATION * self.sample_rate as f32) as usize];
        let mut audio = Vec::new();
        for (index, chunk) in chunks.iter().enumerate() {
            if index > 0 {
                audio.extend_from_slice(&silence);
            }
            let wav = self.synthesize_chunk(chunk, &style, total_steps, speed, lang)?;
            audio.extend_from_slice(&wav);
        }
        Ok((audio, self.sample_rate))
    }

    fn load_style(&self, voice: &str) -> Result<Style> {
        let path = if voice.ends_with(".json") {
            expand_path(voice)
        } else {
            self.voices_dir.join(format!("{voice}.json"))
        };
        let bytes = fs::read(&path).with_context(|| format!("load supertonic voice style '{voice}'"))?;
        let file: StyleFile =
            serde_json::from_slice(&bytes).with_context(|| format!("parse '{}'", path.display()))?;
        Ok(Style {
            style_ttl: flatten_style_tensor(file.style_ttl)?,
            style_dp: flatten_style_tensor(file.style_dp)?,
        })
    }

    fn synthesize_chunk(
        &mut self,
        chunk: &str,
        style: &Style,
        total_steps: usize,
        speed: f32,
        lang: &str,
    ) -> Result<Vec<f32>> {
        let preprocessed = preprocess(chunk, lang);
        let text_ids = index_text(&self.indexer, &preprocessed);
        if text_ids.is_empty() {
            return Ok(Vec::new());
        }
        let length = text_ids.len();
        let text_ids_tensor = Tensor::from_array((vec![1usize, length], text_ids.clone()))?;
        let text_mask = text_mask_tensor(length);
        let style_ttl = style.style_ttl.clone();
        let style_dp = style.style_dp.clone();

        let dur_inputs = ort::inputs![
            "text_ids" => text_ids_tensor.clone(),
            "style_dp" => tensor_f32(style_dp)?,
            "text_mask" => tensor_f32(text_mask.clone())?,
        ];
        let duration = {
            let dur_outputs = self.dp.run(dur_inputs)?;
            let (_, dur_data) = dur_outputs[0].try_extract_tensor::<f32>()?;
            dur_data.first().copied().unwrap_or(0.0) / speed
        };

        let te_inputs = ort::inputs![
            "text_ids" => text_ids_tensor.clone(),
            "style_ttl" => tensor_f32(style_ttl.clone())?,
            "text_mask" => tensor_f32(text_mask.clone())?,
        ];
        let (te_shape, te_data) = {
            let te_outputs = self.text_enc.run(te_inputs)?;
            let (te_shape, te_data) = te_outputs[0].try_extract_tensor::<f32>()?;
            let shape = te_shape.iter().map(|dim| *dim as usize).collect::<Vec<_>>();
            (shape, te_data.to_vec())
        };

        let (mut xt, latent_mask) = self.sample_noisy_latent(duration)?;
        let total_step_value = vec![total_steps as f32];
        for step in 0..total_steps {
            let ve_inputs = ort::inputs![
                "noisy_latent" => tensor_f32(xt.clone())?,
                "text_emb" => tensor_f32((te_shape.clone(), te_data.clone()))?,
                "style_ttl" => tensor_f32(style_ttl.clone())?,
                "latent_mask" => tensor_f32(latent_mask.clone())?,
                "text_mask" => tensor_f32(text_mask.clone())?,
                "current_step" => Tensor::from_array(([1usize], vec![step as f32]))?,
                "total_step" => Tensor::from_array(([1usize], total_step_value.clone()))?,
            ];
            let ve_outputs = self.vector_est.run(ve_inputs)?;
            let (shape, data) = ve_outputs[0].try_extract_tensor::<f32>()?;
            let shape = shape.iter().map(|dim| *dim as usize).collect::<Vec<_>>();
            xt = (shape, data.to_vec());
        }

        let vocoder_outputs = self.vocoder.run(ort::inputs!["latent" => tensor_f32(xt)?])?;
        let (_, wav) = vocoder_outputs[0].try_extract_tensor::<f32>()?;
        Ok(wav.to_vec())
    }

    fn sample_noisy_latent(
        &self,
        duration_seconds: f32,
    ) -> Result<(TensorData, TensorData)> {
        let wav_len_max = duration_seconds * self.sample_rate as f32;
        let wav_lengths = wav_len_max as i64;
        let chunk_size = (self.base_chunk_size * self.chunk_compress_factor) as i64;
        let latent_len = (((wav_len_max + chunk_size as f32 - 1.0) / chunk_size as f32) as i64)
            .max(1) as usize;
        let latent_dim = self.latent_dim * self.chunk_compress_factor;
        let normal = Normal::new(0.0, 1.0)?;
        let mut random = rng();
        let mut noisy = vec![0.0f32; latent_dim * latent_len];
        for value in noisy.iter_mut() {
            *value = normal.sample(&mut random);
        }
        let mask_frames = ((wav_lengths + chunk_size - 1) / chunk_size).max(1) as usize;
        let mut latent_mask = vec![0.0f32; latent_len];
        for (frame, slot) in latent_mask.iter_mut().enumerate() {
            if frame < mask_frames {
                *slot = 1.0;
            }
        }
        for channel in 0..latent_dim {
            for frame in 0..latent_len {
                noisy[channel * latent_len + frame] *= latent_mask[frame];
            }
        }
        Ok((
            (vec![1, latent_dim, latent_len], noisy),
            (vec![1, 1, latent_len], latent_mask),
        ))
    }
}

fn load_session(dir: &Path, stem: &str) -> Result<Session> {
    let path = dir.join(format!("{stem}.onnx"));
    if !path.exists() {
        return Err(anyhow!("Supertonic model '{stem}' not found in {}", dir.display()));
    }
    let builder = Session::builder()
        .map_err(|error| anyhow!("create ONNX session builder: {error}"))?;
    let mut builder = builder
        .with_intra_threads(2)
        .map_err(|error| anyhow!("configure ONNX session: {error}"))?;
    builder = builder
        .with_memory_pattern(false)
        .map_err(|error| anyhow!("disable ONNX memory patterns: {error}"))?
        .with_config_entry("CPUExecutionProvider.use_arena", "0")
        .map_err(|error| anyhow!("disable ONNX CPU arena: {error}"))?;
    Ok(builder.commit_from_file(path)?)
}

fn tensor_f32(value: TensorData) -> Result<Tensor<f32>> {
    Ok(Tensor::from_array(value)?)
}

fn flatten_style_tensor(tensor: StyleTensorFile) -> Result<TensorData> {
    let dims = tensor.dims.clone();
    let data: Vec<f32> = tensor.data.into_iter().flatten().flatten().collect();
    let expected: usize = dims.iter().product();
    if data.len() != expected {
        return Err(anyhow!(
            "voice style tensor has {} values but dims {:?} expect {expected}",
            data.len(),
            dims
        ));
    }
    Ok((dims, data))
}

fn expand_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn index_text(indexer: &[i32], preprocessed: &str) -> Vec<i64> {
    preprocessed
        .chars()
        .map(|c| {
            let code = (c as u32) as usize & 0xFFFF;
            let index = indexer.get(code).copied().unwrap_or(-1);
            index as i64
        })
        .collect()
}

fn text_mask_tensor(length: usize) -> TensorData {
    (vec![1, 1, length], vec![1.0f32; length])
}

fn detect_lang(text: &str) -> &'static str {
    let code = whatlang::detect(text)
        .map(|info| info.lang().code().to_string())
        .unwrap_or_default();
    lang_from_code(&code)
}

fn lang_from_code(code: &str) -> &'static str {
    match code {
        "eng" => "en",
        "kor" => "ko",
        "jpn" => "ja",
        "ara" => "ar",
        "bul" => "bg",
        "ces" => "cs",
        "dan" => "da",
        "deu" => "de",
        "ell" => "el",
        "spa" => "es",
        "est" => "et",
        "fin" => "fi",
        "fra" => "fr",
        "hin" => "hi",
        "hrv" => "hr",
        "hun" => "hu",
        "ind" => "id",
        "ita" => "it",
        "lit" => "lt",
        "lav" => "lv",
        "nld" => "nl",
        "pol" => "pl",
        "por" => "pt",
        "ron" => "ro",
        "rus" => "ru",
        "slk" => "sk",
        "slv" => "sl",
        "swe" => "sv",
        "tur" => "tr",
        "ukr" => "uk",
        "vie" => "vi",
        _ => "na",
    }
}

const EMOJI_RANGES: &[(u32, u32)] = &[
    (0x1f600, 0x1f64f),
    (0x1f300, 0x1f5ff),
    (0x1f680, 0x1f6ff),
    (0x1f700, 0x1f77f),
    (0x1f780, 0x1f7ff),
    (0x1f800, 0x1f8ff),
    (0x1f900, 0x1f9ff),
    (0x1fa00, 0x1fa6f),
    (0x1fa70, 0x1faff),
    (0x2600, 0x26ff),
    (0x2700, 0x27bf),
    (0x1f1e6, 0x1f1ff),
];

fn is_emoji(c: char) -> bool {
    let code = c as u32;
    EMOJI_RANGES
        .iter()
        .any(|(low, high)| code >= *low && code <= *high)
}

fn is_special_symbol(c: char) -> bool {
    matches!(c, '\u{2665}' | '\u{2606}' | '\u{2661}' | '\u{00A9}' | '\\')
}

fn normalize_symbols(c: char) -> char {
    match c {
        '\u{2013}' | '\u{2011}' | '\u{2014}' => '-',
        '\u{00AF}' | '_' | '[' | ']' | '|' | '/' | '#' | '\u{2192}' | '\u{2190}' => ' ',
        '\u{201C}' | '\u{201D}' => '"',
        '\u{2018}' | '\u{2019}' | '\u{00B4}' | '`' => '\'',
        other => other,
    }
}

const ABBREVIATION_EXPANSIONS: &[(&str, &str)] = &[
    ("@", " at "),
    ("e.g.,", "for example, "),
    ("i.e.,", "that is, "),
];

fn fix_punctuation_spacing(text: &str) -> String {
    const SPACED: &[(&str, &str)] = &[
        (" ,", ","),
        (" .", "."),
        (" !", "!"),
        (" ?", "?"),
        (" ;", ";"),
        (" :", ":"),
        (" '", "'"),
    ];
    let mut out = text.to_string();
    for (from, to) in SPACED {
        out = out.replace(from, to);
    }
    out
}

fn dedupe_quotes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous: Option<char> = None;
    for c in text.chars() {
        if matches!(c, '"' | '\'' | '`') && previous == Some(c) {
            continue;
        }
        out.push(c);
        previous = Some(c);
    }
    out
}

fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !in_space {
                out.push(' ');
                in_space = true;
            }
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out.trim().to_string()
}

const ENDING_PUNCTUATION: &[char] = &[
    '.', '!', '?', ';', ':', ',', '\'', '"', ')', ']', '}', '\u{2026}', '\u{3002}', '\u{300D}',
    '\u{300F}', '\u{3011}', '\u{3009}', '\u{300B}', '\u{203A}', '\u{00BB}',
];

fn ends_with_terminator(text: &str) -> bool {
    text.chars()
        .next_back()
        .map(|c| ENDING_PUNCTUATION.contains(&c))
        .unwrap_or(false)
}

fn preprocess(text: &str, lang: &str) -> String {
    let nfkd: String = text.nfkd().collect();
    let no_emoji: String = nfkd.chars().filter(|&c| !is_emoji(c)).collect();
    let normalized: String = no_emoji.chars().map(normalize_symbols).collect();
    let cleaned: String = normalized
        .chars()
        .filter(|&c| !is_special_symbol(c))
        .collect();
    let mut out = cleaned;
    for (from, to) in ABBREVIATION_EXPANSIONS {
        out = out.replace(from, to);
    }
    out = fix_punctuation_spacing(&out);
    out = dedupe_quotes(&out);
    out = collapse_whitespace(&out);
    if !ends_with_terminator(&out) {
        out.push('.');
    }
    format!("<{lang}>{out}</{lang}>")
}

fn is_protected_boundary(text: &str, whitespace_start: usize) -> bool {
    let Some(punct) = text[..whitespace_start].chars().next_back() else {
        return false;
    };
    if punct != '.' {
        return false;
    }
    let dot_position = whitespace_start - 1;
    let mut token_start = dot_position;
    while token_start > 0 {
        let Some(c) = text[..token_start].chars().next_back() else {
            break;
        };
        if c.is_ascii_alphanumeric() || c == '.' {
            token_start -= c.len_utf8();
        } else {
            break;
        }
    }
    let token = &text[token_start..=dot_position];
    const PROTECTED: &[&str] = &[
        "Mr.", "Mrs.", "Ms.", "Dr.", "Prof.", "Sr.", "Jr.", "Ph.D.", "etc.", "e.g.", "i.e.",
        "vs.", "Inc.", "Ltd.", "Co.", "Corp.", "St.", "Ave.", "Blvd.",
    ];
    if PROTECTED.contains(&token) {
        return true;
    }
    let chars: Vec<char> = token.chars().collect();
    if chars.len() >= 2 && chars[chars.len() - 2].is_ascii_uppercase() {
        let boundary = match text[..token_start].chars().next_back() {
            None => true,
            Some(c) => !(c.is_alphanumeric() || c == '_'),
        };
        if boundary {
            return true;
        }
    }
    false
}

fn split_sentences_abbrev(paragraph: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < paragraph.len() {
        let Some(c) = paragraph[index..].chars().next() else {
            break;
        };
        if c.is_whitespace() && index > start {
            let previous = paragraph[..index].chars().next_back().unwrap();
            if matches!(previous, '.' | '!' | '?') && !is_protected_boundary(paragraph, index) {
                sentences.push(paragraph[start..index].to_string());
                let mut next = index + c.len_utf8();
                while next < paragraph.len() {
                    let Some(c2) = paragraph[next..].chars().next() else {
                        break;
                    };
                    if c2.is_whitespace() {
                        next += c2.len_utf8();
                    } else {
                        break;
                    }
                }
                start = next;
                index = next;
                continue;
            }
        }
        index += c.len_utf8();
    }
    if start < paragraph.len() {
        sentences.push(paragraph[start..].to_string());
    }
    sentences
}

fn chunk_text(text: &str, max_len: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    for paragraph in text.split("\n\n") {
        let paragraph = paragraph.trim();
        if paragraph.is_empty() {
            continue;
        }
        let mut current = String::new();
        for sentence in split_sentences_abbrev(paragraph) {
            let sentence = sentence.trim();
            if sentence.is_empty() {
                continue;
            }
            if current.is_empty() {
                current = sentence.to_string();
            } else if current.len() + sentence.len() < max_len {
                current.push(' ');
                current.push_str(sentence);
            } else {
                chunks.push(current);
                current = sentence.to_string();
            }
        }
        if !current.is_empty() {
            chunks.push(current);
        }
    }
    if chunks.is_empty() {
        vec![text.trim().to_string()]
    } else {
        chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_all_supported_language_codes() {
        let expected = [
            "en", "ko", "ja", "ar", "bg", "cs", "da", "de", "el", "es", "et", "fi", "fr", "hi",
            "hr", "hu", "id", "it", "lt", "lv", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv",
            "tr", "uk", "vi",
        ];
        let codes = [
            "eng", "kor", "jpn", "ara", "bul", "ces", "dan", "deu", "ell", "spa", "est", "fin",
            "fra", "hin", "hrv", "hun", "ind", "ita", "lit", "lav", "nld", "pol", "por", "ron",
            "rus", "slk", "slv", "swe", "tur", "ukr", "vie",
        ];
        for (code, want) in codes.iter().zip(expected.iter()) {
            assert_eq!(lang_from_code(code), *want, "code {code}");
        }
        assert_eq!(lang_from_code("nob"), "na");
        assert_eq!(lang_from_code(""), "na");
    }

    #[test]
    fn detects_languages_on_sample_sentences() {
        let samples = [
            ("en", "This is an English sentence for testing purposes."),
            ("ko", "이것은 한국어 테스트 문장입니다."),
            ("ja", "これは日本語のテスト文です。"),
            ("ru", "Это предложение на русском языке для проверки."),
            ("hu", "Ez egy magyar mondat a teszteléshez."),
            ("vi", "Đây là một câu tiếng Việt để kiểm tra."),
            ("uk", "Це речення українською мовою для перевірки."),
            ("ar", "هذه جملة عربية للاختبار."),
            ("el", "Αυτή είναι μια ελληνική πρόταση για δοκιμή."),
            ("hi", "यह हिंदी में परीक्षण वाक्य है।"),
        ];
        for (want, text) in samples {
            assert_eq!(detect_lang(text), want, "text: {text}");
        }
    }

    #[test]
    fn preprocess_wraps_with_lang_token_and_adds_period() {
        assert_eq!(preprocess("Hello world", "en"), "<en>Hello world.</en>");
        assert_eq!(preprocess("Bonjour!", "fr"), "<fr>Bonjour!</fr>");
    }

    #[test]
    fn preprocess_cleans_emoji_symbols_and_whitespace() {
        assert_eq!(
            preprocess("Hi 😀  there", "en"),
            "<en>Hi there.</en>"
        );
        assert_eq!(
            preprocess("Hello   , world", "en"),
            "<en>Hello , world.</en>"
        );
    }

    #[test]
    fn preprocess_expands_abbreviations() {
        assert_eq!(
            preprocess("mail me @ example", "en"),
            "<en>mail me at example.</en>"
        );
        assert_eq!(
            preprocess("Use e.g., this", "en"),
            "<en>Use for example, this.</en>"
        );
    }

    #[test]
    fn preprocess_normalizes_curly_quotes_and_dashes() {
        assert_eq!(preprocess("“quoted” – dash", "en"), "<en>\"quoted\" - dash.</en>");
    }

    #[test]
    fn indexes_text_through_indexer() {
        let indexer = vec![-1; 65536];
        assert_eq!(index_text(&indexer, "abc"), vec![-1, -1, -1]);
        assert_eq!(index_text(&indexer, "<en>x</en>"), vec![-1; 10]);
    }

    #[test]
    fn indexes_lang_token_chars() {
        // '<' = 60 -> 29, '>' = 62 -> 31 in the shipped indexer.
        let mut indexer = vec![-1; 65536];
        indexer[60] = 29;
        indexer[62] = 31;
        indexer[ord('e')] = 64;
        indexer[ord('n')] = 73;
        assert_eq!(index_text(&indexer, "<en>"), vec![29, 64, 73, 31]);
    }

    fn ord(c: char) -> usize {
        c as usize
    }

    #[test]
    fn chunks_text_merges_sentences_up_to_limit() {
        let long = "One. Two. Three.";
        let chunks = chunk_text(long, 10);
        assert_eq!(chunks, vec!["One. Two.", "Three."]);
    }

    #[test]
    fn chunk_text_splits_on_abbreviation_protected_periods() {
        let text = "Dr. Smith went home. He slept.";
        assert_eq!(split_sentences_abbrev(text), vec!["Dr. Smith went home.", "He slept."]);
        assert_eq!(split_sentences_abbrev("Use e.g. here. Done."), vec!["Use e.g. here.", "Done."]);
        assert_eq!(split_sentences_abbrev("U.S. is big. Really."), vec!["U.S. is big.", "Really."]);
    }

    #[test]
    fn style_tensor_flattening_rejects_bad_shapes() {
        let tensor = StyleTensorFile {
            dims: vec![1, 2],
            data: vec![vec![vec![1.0, 2.0]]],
        };
        let flattened = flatten_style_tensor(tensor).unwrap();
        assert_eq!(flattened.0, vec![1, 2]);
        assert_eq!(flattened.1, vec![1.0, 2.0]);

        let bad = StyleTensorFile {
            dims: vec![1, 3],
            data: vec![vec![vec![1.0, 2.0]]],
        };
        assert!(flatten_style_tensor(bad).is_err());
    }
}
