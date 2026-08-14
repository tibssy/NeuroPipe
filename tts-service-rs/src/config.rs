use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub ipc: IpcConfig,
    pub tts: TtsConfig,
}

#[derive(Debug, Deserialize)]
pub struct IpcConfig {
    pub tts_cmd: String,
    pub tts_events: String,
}

#[derive(Debug, Deserialize)]
pub struct TtsConfig {
    pub defaults: Defaults,
    #[serde(default)]
    pub speeds: HashMap<String, f64>,
    #[serde(default)]
    pub qualities: HashMap<String, String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Defaults {
    pub engine: String,
    pub voice: String,
    pub speed: f64,
    pub quality: String,
    pub idle_timeout_sec: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ipc: IpcConfig {
                tts_cmd: "ipc:///tmp/neuropipe_tts_cmd.sock".to_string(),
                tts_events: "ipc:///tmp/neuropipe_tts_events.sock".to_string(),
            },
            tts: TtsConfig {
                defaults: Defaults {
                    engine: "kokoro".to_string(),
                    voice: "af_bella".to_string(),
                    speed: 1.0,
                    quality: "high".to_string(),
                    idle_timeout_sec: 60,
                },
                speeds: HashMap::new(),
                qualities: HashMap::new(),
            },
        }
    }
}

pub fn load() -> Config {
    let path = config_path();
    let Ok(contents) = fs::read_to_string(path) else {
        return Config::default();
    };
    toml::from_str(&contents).unwrap_or_else(|error| {
        eprintln!("[config] invalid config: {error}");
        Config::default()
    })
}

fn config_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".config/neuropipe/config.toml"))
        .unwrap_or_else(|| PathBuf::from("config.toml"))
}
