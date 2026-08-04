use super::{Quality, TtsEngine};
use anyhow::{anyhow, Context, Result};
use ort::session::Session;
use ort::value::{DynTensor, Tensor};
use rand::rng;
use rand_distr::{Distribution, Normal};
use safetensors::{Dtype, SafeTensors};
use sentencepiece_model::SentencePieceModel;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const SAMPLE_RATE: u32 = 24_000;
const MAX_TOKEN_PER_CHUNK: usize = 50;
const EOS_LOGIT_THRESHOLD: f32 = -1.5;
const EOS_TAIL_STEPS: usize = 8;
const MAX_DECODE_STEPS_CAP: usize = 420;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Precision {
    Int8,
    Fp32,
}

impl From<Quality> for Precision {
    fn from(quality: Quality) -> Self {
        match quality {
            Quality::Low => Self::Int8,
            Quality::High => Self::Fp32,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Bundle {
    conditioning_dim: usize,
    frame_rate: f32,
    latent_dim: usize,
    tokenizer_file: String,
    flow_lm_state_manifest: Vec<StateEntry>,
    mimi_state_manifest: Vec<StateEntry>,
}

#[derive(Debug, Deserialize, Clone)]
struct StateEntry {
    dtype: String,
    fill: String,
    index: usize,
    input_name: String,
    key: String,
    module: String,
    #[serde(rename = "output_name")]
    _output_name: String,
    shape: Vec<usize>,
}

enum StateValue {
    F32(Vec<usize>, Vec<f32>),
    I64(Vec<usize>, Vec<i64>),
    Bool(Vec<usize>, Vec<bool>),
}

type State = HashMap<String, StateValue>;

pub struct PocketTtsEngine {
    model_dir: PathBuf,
    quality: Quality,
    inner: Option<PocketTts>,
}

struct PocketTts {
    bundle: Bundle,
    tokenizer: Tokenizer,
    text_conditioner: Session,
    flow_lm_main: Session,
    flow_lm_flow: Session,
    mimi_decoder: Session,
    voices_dir: PathBuf,
}

impl PocketTtsEngine {
    pub fn new(model_dir: impl AsRef<Path>, quality: Quality) -> Self {
        Self {
            model_dir: model_dir.as_ref().to_path_buf(),
            quality,
            inner: None,
        }
    }
}

impl TtsEngine for PocketTtsEngine {
    fn load(&mut self) -> Result<()> {
        if self.inner.is_none() {
            self.inner = Some(PocketTts::load(&self.model_dir, self.quality)?);
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
        self.load()?;
        self.inner.as_ref().unwrap().voices()
    }

    fn synthesize(&mut self, text: &str, voice: &str, speed: f32) -> Result<(Vec<f32>, u32)> {
        self.load()?;
        self.inner.as_mut().unwrap().synthesize(text, voice, speed)
    }
}

impl PocketTts {
    fn load(model_dir: &Path, quality: Quality) -> Result<Self> {
        let bundle_dir = model_dir.join("onnx/english_2026-04");
        let bundle: Bundle =
            serde_json::from_str(&fs::read_to_string(bundle_dir.join("bundle.json"))?)?;
        let precision = Precision::from(quality);
        let tokenizer = Tokenizer::open(bundle_dir.join(&bundle.tokenizer_file))?;
        let text_conditioner = load_session(&bundle_dir, "text_conditioner", precision)?;
        let flow_lm_main = load_session(&bundle_dir, "flow_lm_main", precision)?;
        let flow_lm_flow = load_session(&bundle_dir, "flow_lm_flow", precision)?;
        let mimi_decoder = load_session(&bundle_dir, "mimi_decoder", precision)?;
        let voices_dir = model_dir.join("voices");
        if !voices_dir.is_dir() {
            return Err(anyhow!(
                "PocketTTS voices directory not found: {}",
                voices_dir.display()
            ));
        }
        Ok(Self {
            bundle,
            tokenizer,
            text_conditioner,
            flow_lm_main,
            flow_lm_flow,
            mimi_decoder,
            voices_dir,
        })
    }

    fn voices(&self) -> Result<Vec<String>> {
        let mut voices = fs::read_dir(&self.voices_dir)?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.path().file_name().map(|name| name.to_owned()))
            .filter_map(|name| name.to_str().map(str::to_owned))
            .filter(|name| name.ends_with(".safetensors"))
            .map(|name| name.trim_end_matches(".safetensors").to_string())
            .collect::<Vec<_>>();
        voices.sort();
        Ok(voices)
    }

    fn synthesize(&mut self, text: &str, voice: &str, speed: f32) -> Result<(Vec<f32>, u32)> {
        if !(0.5..=2.0).contains(&speed) {
            return Err(anyhow!("speed must be between 0.5 and 2.0"));
        }
        let voice_path = if voice.ends_with(".safetensors") {
            expand_path(voice)
        } else {
            self.voices_dir.join(format!("{voice}.safetensors"))
        };
        let voice_bytes =
            fs::read(&voice_path).with_context(|| format!("load PocketTTS voice '{voice}'"))?;
        let voice_state = load_voice_state(&voice_bytes, &self.bundle.flow_lm_state_manifest)?;
        let mut audio = Vec::new();
        for sentence in split_text(&self.tokenizer, text) {
            let tokens = self.tokenizer.encode(&sentence);
            let ids = tokens;
            let text_embeddings = self.condition(&ids)?;
            let mut state = clone_state(&voice_state);
            let word_count = sentence.split_whitespace().count().max(1);
            let latents = self.decode_flow(&mut state, &text_embeddings, ids.len(), word_count)?;
            audio.extend(self.decode_audio(&latents)?);
        }
        Ok((change_speed(&audio, speed), SAMPLE_RATE))
    }

    fn condition(&mut self, ids: &[i64]) -> Result<(Vec<usize>, Vec<f32>)> {
        let input = Tensor::from_array((vec![1usize, ids.len()], ids.to_vec()))?;
        let outputs = self
            .text_conditioner
            .run(ort::inputs!["token_ids" => input])?;
        let (shape, data) = outputs[0].try_extract_tensor::<f32>()?;
        let shape = shape.iter().map(|dim| *dim as usize).collect();
        Ok((shape, data.to_vec()))
    }

    fn decode_flow(
        &mut self,
        state: &mut State,
        text_embeddings: &(Vec<usize>, Vec<f32>),
        token_count: usize,
        word_count: usize,
    ) -> Result<Vec<Vec<f32>>> {
        let max_len = ((token_count as f32 / 2.4) * self.bundle.frame_rate
            + 2.5 * self.bundle.frame_rate)
            .ceil()
            .max(
                ((word_count as f32 / 2.2) * self.bundle.frame_rate + 2.0 * self.bundle.frame_rate)
                    .ceil(),
            )
            .min(MAX_DECODE_STEPS_CAP as f32) as usize;
        let min_decode_steps = max_len.min((token_count as f32 * 1.1).ceil().max(12.0) as usize);
        let empty_embeddings = (vec![1, 0, self.bundle.conditioning_dim], Vec::new());
        let initial_sequence = (vec![1, 0, self.bundle.latent_dim], Vec::new());
        let initial = self.flow_inputs(initial_sequence, text_embeddings.clone(), state)?;
        {
            let initial_outputs = self.flow_lm_main.run(initial)?;
            update_state(
                state,
                &initial_outputs,
                &self.bundle.flow_lm_state_manifest,
                2,
            )?;
        }
        let mut sequence = (
            vec![1, 1, self.bundle.latent_dim],
            vec![f32::NAN; self.bundle.latent_dim],
        );
        let mut eos_step = None;
        let mut latents = Vec::new();

        for step in 0..max_len {
            let inputs = self.flow_inputs(sequence.clone(), empty_embeddings.clone(), state)?;
            let (eos, cond_shape, cond_data) = {
                let outputs = self.flow_lm_main.run(inputs)?;
                let (_, eos) = outputs[1].try_extract_tensor::<f32>()?;
                let eos = eos.first().copied().unwrap_or(f32::NEG_INFINITY);
                let cond = outputs[0].try_extract_tensor::<f32>()?;
                let cond_shape = cond.0.iter().map(|dim| *dim as usize).collect::<Vec<_>>();
                let cond_data = cond.1.to_vec();
                update_state(state, &outputs, &self.bundle.flow_lm_state_manifest, 2)?;
                (eos, cond_shape, cond_data)
            };
            if eos > EOS_LOGIT_THRESHOLD && eos_step.is_none() && step >= min_decode_steps / 2 {
                eos_step = Some(step);
            }
            if eos_step.is_some_and(|eos| step >= (eos + EOS_TAIL_STEPS).max(min_decode_steps)) {
                break;
            }
            let normal = Normal::new(0.0, 0.7f32.sqrt())?;
            let mut random = rng();
            let noise = (0..self.bundle.latent_dim)
                .map(|_| normal.sample(&mut random))
                .collect::<Vec<_>>();
            let flow_inputs = ort::inputs![
                "c" => Tensor::from_array((cond_shape, cond_data))?,
                "s" => Tensor::from_array(([1usize, 1usize], vec![0.0f32]))?,
                "t" => Tensor::from_array(([1usize, 1usize], vec![1.0f32]))?,
                "x" => Tensor::from_array(([1usize, self.bundle.latent_dim], noise.clone()))?,
            ];
            let flow_data = {
                let flow = self.flow_lm_flow.run(flow_inputs)?;
                let (_, flow_data) = flow[0].try_extract_tensor::<f32>()?;
                flow_data.to_vec()
            };
            let mut latent = Vec::with_capacity(self.bundle.latent_dim);
            for (index, value) in flow_data.iter().enumerate().take(self.bundle.latent_dim) {
                latent.push(*value + noise[index]);
            }
            sequence = (vec![1, 1, self.bundle.latent_dim], latent.clone());
            latents.push(latent);
        }
        Ok(latents)
    }

    fn flow_inputs(
        &self,
        sequence: (Vec<usize>, Vec<f32>),
        text_embeddings: (Vec<usize>, Vec<f32>),
        state: &State,
    ) -> Result<HashMap<String, DynTensor>> {
        let mut inputs = HashMap::new();
        inputs.insert("sequence".to_string(), tensor_f32(sequence)?.upcast());
        inputs.insert(
            "text_embeddings".to_string(),
            tensor_f32(text_embeddings)?.upcast(),
        );
        for entry in &self.bundle.flow_lm_state_manifest {
            let value = state.get(&entry.input_name).context("missing flow state")?;
            inputs.insert(entry.input_name.clone(), value.tensor()?);
        }
        Ok(inputs)
    }

    fn decode_audio(&mut self, latents: &[Vec<f32>]) -> Result<Vec<f32>> {
        let mut state = init_state(&self.bundle.mimi_state_manifest)?;
        let mut audio = Vec::new();
        for chunk in latents.chunks(15) {
            let data = chunk.iter().flatten().copied().collect::<Vec<_>>();
            let inputs = {
                let mut inputs = HashMap::new();
                inputs.insert(
                    "latent".to_string(),
                    Tensor::from_array((vec![1usize, chunk.len(), self.bundle.latent_dim], data))?
                        .upcast(),
                );
                for entry in &self.bundle.mimi_state_manifest {
                    let value = state.get(&entry.input_name).context("missing Mimi state")?;
                    inputs.insert(entry.input_name.clone(), value.tensor()?);
                }
                inputs
            };
            let outputs = self.mimi_decoder.run(inputs)?;
            let (_, samples) = outputs[0].try_extract_tensor::<f32>()?;
            audio.extend_from_slice(samples);
            update_state(&mut state, &outputs, &self.bundle.mimi_state_manifest, 1)?;
        }
        Ok(audio)
    }
}

fn expand_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn load_session(dir: &Path, stem: &str, precision: Precision) -> Result<Session> {
    let preferred = match precision {
        Precision::Int8 => format!("{stem}_int8.onnx"),
        Precision::Fp32 => format!("{stem}.onnx"),
    };
    let fallback = match precision {
        Precision::Int8 => format!("{stem}.onnx"),
        Precision::Fp32 => format!("{stem}_int8.onnx"),
    };
    let path = [dir.join(preferred), dir.join(fallback)]
        .into_iter()
        .find(|path| path.exists())
        .ok_or_else(|| anyhow!("PocketTTS model '{stem}' not found in {}", dir.display()))?;
    let builder =
        Session::builder().map_err(|error| anyhow!("create ONNX session builder: {error}"))?;
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

fn tensor_f32(value: (Vec<usize>, Vec<f32>)) -> Result<Tensor<f32>> {
    Ok(Tensor::from_array(value)?)
}

fn init_state(manifest: &[StateEntry]) -> Result<State> {
    let mut state = State::new();
    for entry in manifest {
        state.insert(entry.input_name.clone(), filled_value(entry)?);
    }
    Ok(state)
}

fn clone_state(state: &State) -> State {
    state
        .iter()
        .map(|(key, value)| (key.clone(), value.clone_value()))
        .collect()
}

fn load_voice_state(bytes: &[u8], manifest: &[StateEntry]) -> Result<State> {
    let tensors = SafeTensors::deserialize(bytes)?;
    let mut state = init_state(manifest)?;
    for entry in manifest {
        let key = format!("{}/{}", entry.module, entry.key);
        if let Ok(tensor) = tensors.tensor(&key) {
            state.insert(entry.input_name.clone(), convert_tensor(&tensor, entry)?);
        } else if entry.key == "step" {
            let offset_key = format!("{}/offset", entry.module);
            if let Ok(offset) = tensors.tensor(&offset_key) {
                state.insert(entry.input_name.clone(), convert_tensor(&offset, entry)?);
            }
        }
    }
    Ok(state)
}

fn convert_tensor(
    tensor: &safetensors::tensor::TensorView<'_>,
    entry: &StateEntry,
) -> Result<StateValue> {
    let shape = entry.shape.clone();
    match tensor.dtype() {
        Dtype::F32 => Ok(StateValue::F32(
            shape,
            copy_prefix_f32(
                tensor.data(),
                tensor.shape(),
                &entry.shape,
                if entry.fill == "nan" { f32::NAN } else { 0.0 },
            ),
        )),
        Dtype::I64 => Ok(StateValue::I64(
            shape,
            copy_prefix_i64(tensor.data(), tensor.shape(), &entry.shape),
        )),
        Dtype::BOOL => Ok(StateValue::Bool(
            shape,
            copy_prefix_bool(tensor.data(), tensor.shape(), &entry.shape),
        )),
        dtype => Err(anyhow!("unsupported PocketTTS voice dtype {dtype:?}")),
    }
}

fn filled_value(entry: &StateEntry) -> Result<StateValue> {
    let count = entry.shape.iter().product();
    match entry.dtype.as_str() {
        "float32" => Ok(StateValue::F32(
            entry.shape.clone(),
            vec![
                if entry.fill == "nan" {
                    f32::NAN
                } else if entry.fill == "ones" {
                    1.0
                } else {
                    0.0
                };
                count
            ],
        )),
        "int64" => Ok(StateValue::I64(entry.shape.clone(), vec![0; count])),
        "bool" => Ok(StateValue::Bool(
            entry.shape.clone(),
            vec![entry.fill == "ones"; count],
        )),
        dtype => Err(anyhow!("unsupported PocketTTS state dtype {dtype}")),
    }
}

impl StateValue {
    fn clone_value(&self) -> Self {
        match self {
            Self::F32(shape, data) => Self::F32(shape.clone(), data.clone()),
            Self::I64(shape, data) => Self::I64(shape.clone(), data.clone()),
            Self::Bool(shape, data) => Self::Bool(shape.clone(), data.clone()),
        }
    }

    fn tensor(&self) -> Result<DynTensor> {
        Ok(match self {
            Self::F32(shape, data) => Tensor::from_array((shape.clone(), data.clone()))?.upcast(),
            Self::I64(shape, data) => Tensor::from_array((shape.clone(), data.clone()))?.upcast(),
            Self::Bool(shape, data) => Tensor::from_array((shape.clone(), data.clone()))?.upcast(),
        })
    }
}

fn update_state(
    state: &mut State,
    outputs: &ort::session::SessionOutputs<'_>,
    manifest: &[StateEntry],
    offset: usize,
) -> Result<()> {
    for entry in manifest {
        let output = &outputs[offset + entry.index];
        let value = match entry.dtype.as_str() {
            "float32" => {
                let (shape, data) = output.try_extract_tensor::<f32>()?;
                StateValue::F32(
                    shape.iter().map(|dim| *dim as usize).collect(),
                    data.to_vec(),
                )
            }
            "int64" => {
                let (shape, data) = output.try_extract_tensor::<i64>()?;
                StateValue::I64(
                    shape.iter().map(|dim| *dim as usize).collect(),
                    data.to_vec(),
                )
            }
            "bool" => {
                let (shape, data) = output.try_extract_tensor::<bool>()?;
                StateValue::Bool(
                    shape.iter().map(|dim| *dim as usize).collect(),
                    data.to_vec(),
                )
            }
            dtype => return Err(anyhow!("unsupported PocketTTS state dtype {dtype}")),
        };
        state.insert(entry.input_name.clone(), value);
    }
    Ok(())
}

fn copy_prefix_f32(
    data: &[u8],
    source_shape: &[usize],
    target_shape: &[usize],
    fill: f32,
) -> Vec<f32> {
    let source = data
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect::<Vec<_>>();
    copy_prefix(&source, source_shape, target_shape, fill)
}

fn copy_prefix_i64(data: &[u8], source_shape: &[usize], target_shape: &[usize]) -> Vec<i64> {
    let source = data
        .chunks_exact(8)
        .map(|b| i64::from_le_bytes(b.try_into().unwrap()))
        .collect::<Vec<_>>();
    copy_prefix(&source, source_shape, target_shape, 0)
}

fn copy_prefix_bool(data: &[u8], source_shape: &[usize], target_shape: &[usize]) -> Vec<bool> {
    let source = data.iter().map(|value| *value != 0).collect::<Vec<_>>();
    copy_prefix(&source, source_shape, target_shape, false)
}

fn copy_prefix<T: Clone>(
    source: &[T],
    source_shape: &[usize],
    target_shape: &[usize],
    fill: T,
) -> Vec<T> {
    if source_shape == target_shape {
        return source.to_vec();
    }
    let mut target = vec![fill; target_shape.iter().product()];
    for target_index in 0..target.len() {
        let mut remainder = target_index;
        let mut source_index = 0;
        let mut source_stride = 1;
        let mut valid = true;
        for axis in (0..target_shape.len()).rev() {
            let coordinate = remainder % target_shape[axis];
            remainder /= target_shape[axis];
            if coordinate >= source_shape.get(axis).copied().unwrap_or(0) {
                valid = false;
                break;
            }
            source_index += coordinate * source_stride;
            source_stride *= source_shape[axis];
        }
        if valid {
            target[target_index] = source[source_index].clone();
        }
    }
    target
}

fn split_text(tokenizer: &Tokenizer, text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if tokenizer.encode(&candidate).len() > MAX_TOKEN_PER_CHUNK && !current.is_empty() {
            chunks.push(current);
            current = word.to_string();
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        vec![text.trim().to_string()]
    } else {
        chunks
    }
}

struct Tokenizer {
    pieces: Vec<Piece>,
}

struct Piece {
    piece: String,
    id: i64,
}

impl Tokenizer {
    fn open(path: impl AsRef<Path>) -> Result<Self> {
        let model = SentencePieceModel::from_file(path)?;
        let pieces = model
            .pieces()
            .into_iter()
            .enumerate()
            .map(|(id, piece)| Piece {
                piece: piece.piece.clone().unwrap_or_default(),
                id: id as i64,
            })
            .collect();
        Ok(Self { pieces })
    }

    fn encode(&self, text: &str) -> Vec<i64> {
        let normalized = format!("▁{}", text.replace(' ', "▁"));
        let chars: Vec<char> = normalized.chars().collect();
        let mut result = Vec::new();
        let mut index = 0;
        while index < chars.len() {
            let mut best: Option<(&Piece, usize)> = None;
            for piece in &self.pieces {
                let piece_chars: Vec<char> = piece.piece.chars().collect();
                if piece_chars.is_empty()
                    || index + piece_chars.len() > chars.len()
                    || chars[index..index + piece_chars.len()] != piece_chars
                {
                    continue;
                }
                if best
                    .map(|(_, length)| piece_chars.len() > length)
                    .unwrap_or(true)
                {
                    best = Some((piece, piece_chars.len()));
                }
            }
            if let Some((piece, length)) = best {
                result.push(piece.id);
                index += length;
            } else {
                index += 1;
            }
        }
        result
    }
}

fn change_speed(audio: &[f32], speed: f32) -> Vec<f32> {
    if speed == 1.0 || audio.is_empty() {
        return audio.to_vec();
    }
    let new_len = (audio.len() as f32 / speed) as usize;
    (0..new_len)
        .map(|index| {
            let position = index as f32 * (audio.len() - 1) as f32 / (new_len - 1).max(1) as f32;
            let left = position.floor() as usize;
            let right = (left + 1).min(audio.len() - 1);
            audio[left] + (audio[right] - audio[left]) * (position - left as f32)
        })
        .collect()
}
