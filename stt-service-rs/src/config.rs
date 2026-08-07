use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub ipc: IpcConfig,
    pub stt: SttConfig,
}

#[derive(Debug, Deserialize)]
pub struct IpcConfig {
    pub stt_cmd: String,
    pub stt_pub: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SttConfig {
    pub mode: String,
    pub model: String,
    pub model_dir: String,
    pub vad_threshold: f32,
    pub digital_gain: f32,
    pub silence_timeout_sec: f32,
    pub model_idle_timeout_sec: u64,
    /// Enable smart turn-end detection (replaces the fixed silence timeout).
    pub turn_end_enabled: bool,
    /// Silence a speaker must hold before the turn-end detector is consulted.
    pub turn_hold_ms: u64,
    /// P(end-of-turn) above which the turn is finalized early.
    pub turn_end_threshold: f32,
    /// How often to re-score while the speaker keeps pausing.
    pub turn_score_cadence_ms: u64,
    /// Absolute silence ceiling; the turn always ends at this point.
    pub turn_hard_ceiling_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ipc: IpcConfig {
                stt_cmd: "ipc:///tmp/neuropipe_cmd.sock".to_string(),
                stt_pub: "ipc:///tmp/neuropipe_pub.sock".to_string(),
            },
            stt: SttConfig {
                mode: "IDLE".to_string(),
                model: "nemo-parakeet-tdt-0.6b-v3".to_string(),
                model_dir: "~/.local/share/neuropipe/stt/parakeet-v3".to_string(),
                vad_threshold: 0.5,
                digital_gain: 3.0,
                silence_timeout_sec: 1.0,
                model_idle_timeout_sec: 60,
                turn_end_enabled: true,
                turn_hold_ms: 250,
                turn_end_threshold: 0.5,
                turn_score_cadence_ms: 400,
                turn_hard_ceiling_ms: 3500,
            },
        }
    }
}

fn expand_home(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join(rest)
                .to_string_lossy()
                .into_owned();
        }
    }
    path.to_string()
}

impl Config {
    pub fn stt_model_dir(&self) -> PathBuf {
        PathBuf::from(expand_home(&self.stt.model_dir))
    }

    /// Path to the Silero VAD ONNX model, stored alongside the model dir.
    pub fn vad_path(&self) -> PathBuf {
        let dir = self.stt_model_dir();
        dir.parent()
            .map(|p| p.join("silero_vad.onnx"))
            .unwrap_or_else(|| dir.join("silero_vad.onnx"))
    }
}

pub fn load() -> Config {
    let path = config_path();
    let Ok(contents) = fs::read_to_string(&path) else {
        return Config::default();
    };
    // User config only overrides ipc + stt sections; merge onto defaults.
    let user: UserConfig = toml::from_str(&contents).unwrap_or_else(|error| {
        eprintln!("[config] invalid {path:?}: {error}");
        UserConfig::default()
    });
    let mut base = Config::default();
    if let Some(ipc) = user.ipc {
        base.ipc = ipc;
    }
    if let Some(stt) = user.stt {
        if let Some(v) = stt.mode {
            base.stt.mode = v;
        }
        if let Some(v) = stt.model {
            base.stt.model = v;
        }
        if let Some(v) = stt.model_dir {
            base.stt.model_dir = v;
        }
        if let Some(v) = stt.vad_threshold {
            base.stt.vad_threshold = v;
        }
        if let Some(v) = stt.digital_gain {
            base.stt.digital_gain = v;
        }
        if let Some(v) = stt.silence_timeout_sec {
            base.stt.silence_timeout_sec = v;
        }
        if let Some(v) = stt.model_idle_timeout_sec {
            base.stt.model_idle_timeout_sec = v;
        }
        if let Some(v) = stt.turn_end_enabled {
            base.stt.turn_end_enabled = v;
        }
        if let Some(v) = stt.turn_hold_ms {
            base.stt.turn_hold_ms = v;
        }
        if let Some(v) = stt.turn_end_threshold {
            base.stt.turn_end_threshold = v;
        }
        if let Some(v) = stt.turn_score_cadence_ms {
            base.stt.turn_score_cadence_ms = v;
        }
        if let Some(v) = stt.turn_hard_ceiling_ms {
            base.stt.turn_hard_ceiling_ms = v;
        }
    }
    base
}

#[derive(Debug, Deserialize, Default)]
struct UserConfig {
    ipc: Option<IpcConfig>,
    stt: Option<UserSttConfig>,
}

#[derive(Debug, Deserialize, Default)]
struct UserSttConfig {
    mode: Option<String>,
    model: Option<String>,
    model_dir: Option<String>,
    vad_threshold: Option<f32>,
    digital_gain: Option<f32>,
    silence_timeout_sec: Option<f32>,
    model_idle_timeout_sec: Option<u64>,
    turn_end_enabled: Option<bool>,
    turn_hold_ms: Option<u64>,
    turn_end_threshold: Option<f32>,
    turn_score_cadence_ms: Option<u64>,
    turn_hard_ceiling_ms: Option<u64>,
}

fn config_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".config/neuropipe/config.toml"))
        .unwrap_or_else(|| PathBuf::from("config.toml"))
}
