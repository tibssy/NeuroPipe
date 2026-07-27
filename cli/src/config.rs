use std::{
    env,
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value as JsonValue;
use toml::{map::Map as TomlMap, Value as TomlValue};

const DEFAULT_CONFIG: &str = r#"
version = 1

[ipc]
stt_cmd = "ipc:///tmp/neuropipe_cmd.sock"
stt_pub = "ipc:///tmp/neuropipe_pub.sock"
tts_cmd = "ipc:///tmp/neuropipe_tts_cmd.sock"
tts_events = "ipc:///tmp/neuropipe_tts_events.sock"
assistant_cmd = "ipc:///tmp/neuropipe_assistant_cmd.sock"

[stt]
mode = "IDLE"
model = "nemo-parakeet-tdt-0.6b-v3"
vad_threshold = 0.5
digital_gain = 3.0
model_idle_timeout_sec = 60

[tts.defaults]
engine = "kokoro"
voice = "af_bella"
speed = 1.0
quality = "high"
idle_timeout_sec = 60

[assistant]
default_model = "llama3.2:1b"
history_idle_timeout_sec = 3600

[assistant.instructions]
system_prompt = """
You are a helpful AI voice assistant.
Keep answers short and conversational.
/set nothink
"""
tool_usage_policy = """
When the user asks about something a tool can help with,
call the appropriate tool automatically.
Do not ask for permission.
"""
"#;

fn config_path() -> Result<PathBuf, String> {
    let home = env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
    Ok(PathBuf::from(home).join(".config/neuropipe/config.toml"))
}

fn load_config_doc() -> Result<TomlValue, String> {
    let default_cfg = DEFAULT_CONFIG
        .parse::<TomlValue>()
        .map_err(|e| format!("Failed to parse built-in default config: {e}"))?;

    let path = config_path()?;
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Ok(default_cfg),
    };

    match text.parse::<TomlValue>() {
        Ok(cfg) => Ok(cfg),
        Err(e) => {
            eprintln!("Warning: failed to parse {}: {}", path.display(), e);
            Ok(default_cfg)
        }
    }
}

fn ensure_table<'a>(map: &'a mut TomlMap<String, TomlValue>, key: &str) -> &'a mut TomlMap<String, TomlValue> {
    if !matches!(map.get(key), Some(TomlValue::Table(_))) {
        map.insert(key.to_string(), TomlValue::Table(TomlMap::new()));
    }
    map.get_mut(key)
        .and_then(TomlValue::as_table_mut)
        .expect("table just inserted")
}

fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("No parent directory for {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = path.with_extension(format!("tmp.{}.{}", std::process::id(), ts));

    let mut f = fs::File::create(&tmp).map_err(|e| format!("Failed to create {}: {e}", tmp.display()))?;
    f.write_all(content.as_bytes())
        .map_err(|e| format!("Failed to write {}: {e}", tmp.display()))?;
    f.sync_all()
        .map_err(|e| format!("Failed to fsync {}: {e}", tmp.display()))?;

    fs::rename(&tmp, path)
        .map_err(|e| format!("Failed to replace {}: {e}", path.display()))?;

    let dir = fs::File::open(parent).map_err(|e| format!("Failed to open {}: {e}", parent.display()))?;
    dir.sync_all()
        .map_err(|e| format!("Failed to fsync {}: {e}", parent.display()))?;
    Ok(())
}

fn save_config_doc(cfg: &TomlValue) -> Result<(), String> {
    let text = toml::to_string_pretty(cfg).map_err(|e| format!("Failed to encode config TOML: {e}"))?;
    let path = config_path()?;
    write_atomic(&path, &text)
}

pub fn persist_tts_defaults(engine: &str, voice: &str, speed: f64, quality: &str) -> Result<(), String> {
    if !matches!(engine, "kokoro" | "pocket-tts") {
        return Err(format!("Invalid engine '{}'.", engine));
    }
    if !matches!(quality, "low" | "high") {
        return Err(format!("Invalid quality '{}'.", quality));
    }
    if !(0.5..=2.0).contains(&speed) {
        return Err(format!("Invalid speed '{}'.", speed));
    }
    if voice.trim().is_empty() {
        return Err("Voice must be non-empty.".to_string());
    }

    let mut cfg = load_config_doc()?;
    let root = cfg
        .as_table_mut()
        .ok_or_else(|| "Config root is not a TOML table".to_string())?;
    let tts = ensure_table(root, "tts");
    let defaults = ensure_table(tts, "defaults");

    defaults.insert("engine".to_string(), TomlValue::String(engine.to_string()));
    defaults.insert("voice".to_string(), TomlValue::String(voice.to_string()));
    defaults.insert("speed".to_string(), TomlValue::Float(speed));
    defaults.insert("quality".to_string(), TomlValue::String(quality.to_string()));

    save_config_doc(&cfg)
}

pub fn persist_assistant_model(model: &str) -> Result<(), String> {
    if model.trim().is_empty() {
        return Err("Model must be non-empty.".to_string());
    }

    let mut cfg = load_config_doc()?;
    let root = cfg
        .as_table_mut()
        .ok_or_else(|| "Config root is not a TOML table".to_string())?;
    let assistant = ensure_table(root, "assistant");
    assistant.insert("default_model".to_string(), TomlValue::String(model.to_string()));

    save_config_doc(&cfg)
}

pub fn persist_assistant_tools(tools: &JsonValue) -> Result<(), String> {
    let obj = tools
        .as_object()
        .ok_or_else(|| "tools reply is not an object".to_string())?;

    let mut table = TomlMap::new();
    for (name, level_value) in obj {
        let level = level_value
            .as_str()
            .ok_or_else(|| format!("tool '{name}' permission must be a string"))?;
        if !matches!(level, "allow" | "ask" | "deny") {
            return Err(format!("Invalid permission '{level}' for tool '{name}'"));
        }
        table.insert(name.clone(), TomlValue::String(level.to_string()));
    }

    let mut cfg = load_config_doc()?;
    let root = cfg
        .as_table_mut()
        .ok_or_else(|| "Config root is not a TOML table".to_string())?;
    let assistant = ensure_table(root, "assistant");
    assistant.insert("tools".to_string(), TomlValue::Table(table));

    save_config_doc(&cfg)
}
