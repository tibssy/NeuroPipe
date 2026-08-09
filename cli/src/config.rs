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
silence_timeout_sec = 1.0
model_idle_timeout_sec = 60

[tts.defaults]
engine = "kokoro"
voice = "af_bella"
speed = 1.0
quality = "high"
idle_timeout_sec = 60

[tts.favorites]
kokoro = []
pocket_tts = []
supertonic_3 = []

[assistant]
default_model = "llama3.2:1b"
history_idle_timeout_sec = 3600

[assistant.favorites]
models = []

[assistant.memory]
enabled_local = true
enabled_cloud = false
summarize_on_idle = true
summarize_on_stop = true
max_summary_chars = 1200
retrieve_top_k = 4
qdrant_path = "~/.local/share/neuropipe/memory/qdrant"
collection = "assistant_memory"
embedding_model = "all-minilm"

[assistant.instructions]
system_prompt = """
You are a helpful AI voice assistant.
Keep answers short and conversational.
This is a voice-to-voice conversation: assume the user replies by speaking, not typing.
If you need confirmation (for example before using a tool in ask mode), request a spoken yes/no response and never ask the user to type.
/set nothink
"""
tool_usage_policy = """
When the user asks about something a tool can help with,
call the appropriate tool automatically.
If a tool is in ask mode, request spoken permission (yes/no)
and continue based on the user's voice response.
Do not ask the user to type permission commands.
"""

[assistant.tools]
open_url = "ask"
screenshot = "ask"
web_search = "ask"
"#;

fn config_path() -> Result<PathBuf, String> {
    let home = env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
    Ok(PathBuf::from(home).join(".config/neuropipe/config.toml"))
}

fn parse_default_config() -> Result<TomlValue, String> {
    DEFAULT_CONFIG
        .parse::<TomlValue>()
        .map_err(|e| format!("Failed to parse built-in default config: {e}"))
}

fn read_raw_config_doc() -> Result<Option<TomlValue>, String> {
    let path = config_path()?;
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Failed to read {}: {e}", path.display())),
    };
    let parsed = text
        .parse::<TomlValue>()
        .map_err(|e| format!("Failed to parse {}: {e}", path.display()))?;
    Ok(Some(parsed))
}

fn merge_toml(defaults: &TomlValue, incoming: &TomlValue) -> TomlValue {
    match (defaults, incoming) {
        (TomlValue::Table(default_map), TomlValue::Table(incoming_map)) => {
            let mut merged = default_map.clone();
            for (key, value) in incoming_map {
                let next = if let Some(existing) = merged.get(key) {
                    merge_toml(existing, value)
                } else {
                    value.clone()
                };
                merged.insert(key.clone(), next);
            }
            TomlValue::Table(merged)
        }
        (_, incoming_value) => incoming_value.clone(),
    }
}

fn effective_config_doc() -> Result<TomlValue, String> {
    let defaults = parse_default_config()?;
    match read_raw_config_doc()? {
        Some(raw) => Ok(merge_toml(&defaults, &raw)),
        None => Ok(defaults),
    }
}

fn load_config_doc() -> Result<TomlValue, String> {
    let default_cfg = parse_default_config()?;

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

fn require_table<'a>(cfg: &'a TomlValue, path: &str) -> Result<&'a TomlMap<String, TomlValue>, String> {
    cfg.as_table()
        .ok_or_else(|| format!("{path} must be a table"))
}

fn require_string(table: &TomlMap<String, TomlValue>, key: &str, path: &str) -> Result<String, String> {
    table
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("{path}.{key} must be a string"))
}

fn require_int(table: &TomlMap<String, TomlValue>, key: &str, path: &str) -> Result<i64, String> {
    table
        .get(key)
        .and_then(|v| v.as_integer())
        .ok_or_else(|| format!("{path}.{key} must be an integer"))
}

fn require_bool(table: &TomlMap<String, TomlValue>, key: &str, path: &str) -> Result<bool, String> {
    table
        .get(key)
        .and_then(|v| v.as_bool())
        .ok_or_else(|| format!("{path}.{key} must be a boolean"))
}

fn require_float(table: &TomlMap<String, TomlValue>, key: &str, path: &str) -> Result<f64, String> {
    table
        .get(key)
        .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
        .ok_or_else(|| format!("{path}.{key} must be a number"))
}

fn require_string_array(
    table: &TomlMap<String, TomlValue>,
    key: &str,
    path: &str,
) -> Result<Vec<String>, String> {
    let arr = table
        .get(key)
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("{path}.{key} must be an array"))?;
    let mut out = Vec::with_capacity(arr.len());
    for (idx, item) in arr.iter().enumerate() {
        let Some(value) = item.as_str() else {
            return Err(format!("{path}.{key}[{idx}] must be a string"));
        };
        if value.trim().is_empty() {
            return Err(format!("{path}.{key}[{idx}] must be non-empty"));
        }
        out.push(value.to_string());
    }
    Ok(out)
}

pub fn validate_document(cfg: &TomlValue) -> Result<(), String> {
    let root = require_table(cfg, "root")?;

    let version = require_int(root, "version", "root")?;
    if version < 1 {
        return Err("root.version must be >= 1".to_string());
    }

    let ipc = root
        .get("ipc")
        .and_then(|v| v.as_table())
        .ok_or_else(|| "root.ipc must be a table".to_string())?;
    for key in ["stt_cmd", "stt_pub", "tts_cmd", "tts_events", "assistant_cmd"] {
        let value = require_string(ipc, key, "root.ipc")?;
        if !value.starts_with("ipc://") {
            return Err(format!("root.ipc.{key} must start with 'ipc://'"));
        }
    }

    let stt = root
        .get("stt")
        .and_then(|v| v.as_table())
        .ok_or_else(|| "root.stt must be a table".to_string())?;
    let vad = require_float(stt, "vad_threshold", "root.stt")?;
    if !(0.0..=1.0).contains(&vad) {
        return Err("root.stt.vad_threshold must be in [0.0, 1.0]".to_string());
    }
    let gain = require_float(stt, "digital_gain", "root.stt")?;
    if gain <= 0.0 {
        return Err("root.stt.digital_gain must be > 0".to_string());
    }
    let silence_timeout = require_float(stt, "silence_timeout_sec", "root.stt")?;
    if silence_timeout <= 0.0 {
        return Err("root.stt.silence_timeout_sec must be > 0".to_string());
    }
    let stt_idle = require_int(stt, "model_idle_timeout_sec", "root.stt")?;
    if stt_idle < 1 {
        return Err("root.stt.model_idle_timeout_sec must be >= 1".to_string());
    }

    let tts = root
        .get("tts")
        .and_then(|v| v.as_table())
        .ok_or_else(|| "root.tts must be a table".to_string())?;
    let defaults = tts
        .get("defaults")
        .and_then(|v| v.as_table())
        .ok_or_else(|| "root.tts.defaults must be a table".to_string())?;
    let engine = require_string(defaults, "engine", "root.tts.defaults")?;
    if !matches!(engine.as_str(), "kokoro" | "pocket-tts" | "supertonic-3") {
        return Err(
            "root.tts.defaults.engine must be 'kokoro', 'pocket-tts', or 'supertonic-3'"
                .to_string(),
        );
    }
    let quality = require_string(defaults, "quality", "root.tts.defaults")?;
    if !matches!(quality.as_str(), "low" | "high") {
        return Err("root.tts.defaults.quality must be 'low' or 'high'".to_string());
    }
    let speed = require_float(defaults, "speed", "root.tts.defaults")?;
    if !(0.5..=2.0).contains(&speed) {
        return Err("root.tts.defaults.speed must be in [0.5, 2.0]".to_string());
    }
    let voice = require_string(defaults, "voice", "root.tts.defaults")?;
    if voice.trim().is_empty() {
        return Err("root.tts.defaults.voice must be non-empty".to_string());
    }
    let tts_idle = require_int(defaults, "idle_timeout_sec", "root.tts.defaults")?;
    if tts_idle < 1 {
        return Err("root.tts.defaults.idle_timeout_sec must be >= 1".to_string());
    }

    if let Some(tts_favorites_value) = tts.get("favorites") {
        let tts_favorites = tts_favorites_value
            .as_table()
            .ok_or_else(|| "root.tts.favorites must be a table".to_string())?;
        for key in tts_favorites.keys() {
            if key != "kokoro"
                && key != "pocket_tts"
                && key != "pocket-tts"
                && key != "supertonic_3"
                && key != "supertonic-3"
            {
                return Err(format!("root.tts.favorites has unknown key '{key}'"));
            }
        }
        if tts_favorites.contains_key("kokoro") {
            let _ = require_string_array(tts_favorites, "kokoro", "root.tts.favorites")?;
        }
        if tts_favorites.contains_key("pocket_tts") {
            let _ = require_string_array(tts_favorites, "pocket_tts", "root.tts.favorites")?;
        }
        if tts_favorites.contains_key("pocket-tts") {
            let _ = require_string_array(tts_favorites, "pocket-tts", "root.tts.favorites")?;
        }
        if tts_favorites.contains_key("supertonic_3") {
            let _ = require_string_array(tts_favorites, "supertonic_3", "root.tts.favorites")?;
        }
        if tts_favorites.contains_key("supertonic-3") {
            let _ = require_string_array(tts_favorites, "supertonic-3", "root.tts.favorites")?;
        }
    }

    let assistant = root
        .get("assistant")
        .and_then(|v| v.as_table())
        .ok_or_else(|| "root.assistant must be a table".to_string())?;
    let model = require_string(assistant, "default_model", "root.assistant")?;
    if model.trim().is_empty() {
        return Err("root.assistant.default_model must be non-empty".to_string());
    }
    let history_idle = require_int(assistant, "history_idle_timeout_sec", "root.assistant")?;
    if history_idle < 1 {
        return Err("root.assistant.history_idle_timeout_sec must be >= 1".to_string());
    }

    let memory = assistant
        .get("memory")
        .and_then(|v| v.as_table())
        .ok_or_else(|| "root.assistant.memory must be a table".to_string())?;
    for key in ["enabled_local", "enabled_cloud", "summarize_on_idle", "summarize_on_stop"] {
        let _ = require_bool(memory, key, "root.assistant.memory")?;
    }
    let max_summary = require_int(memory, "max_summary_chars", "root.assistant.memory")?;
    if max_summary < 200 {
        return Err("root.assistant.memory.max_summary_chars must be >= 200".to_string());
    }
    let top_k = require_int(memory, "retrieve_top_k", "root.assistant.memory")?;
    if !(1..=20).contains(&top_k) {
        return Err("root.assistant.memory.retrieve_top_k must be in [1, 20]".to_string());
    }
    for key in ["qdrant_path", "collection", "embedding_model"] {
        let value = require_string(memory, key, "root.assistant.memory")?;
        if value.trim().is_empty() {
            return Err(format!("root.assistant.memory.{key} must be non-empty"));
        }
    }

    let instructions = assistant
        .get("instructions")
        .and_then(|v| v.as_table())
        .ok_or_else(|| "root.assistant.instructions must be a table".to_string())?;
    for key in ["system_prompt", "tool_usage_policy"] {
        let value = require_string(instructions, key, "root.assistant.instructions")?;
        if value.trim().is_empty() {
            return Err(format!("root.assistant.instructions.{key} must be non-empty"));
        }
    }

    let tools = assistant
        .get("tools")
        .and_then(|v| v.as_table())
        .ok_or_else(|| "root.assistant.tools must be a table".to_string())?;
    for (name, level_value) in tools {
        let Some(level) = level_value.as_str() else {
            return Err(format!("root.assistant.tools.{name} must be a string"));
        };
        if !matches!(level, "allow" | "ask" | "deny") {
            return Err(format!("Invalid permission '{level}' for tool '{name}'"));
        }
    }

    if let Some(assistant_favorites_value) = assistant.get("favorites") {
        let assistant_favorites = assistant_favorites_value
            .as_table()
            .ok_or_else(|| "root.assistant.favorites must be a table".to_string())?;
        for key in assistant_favorites.keys() {
            if key != "models" {
                return Err(format!("root.assistant.favorites has unknown key '{key}'"));
            }
        }
        if assistant_favorites.contains_key("models") {
            let _ = require_string_array(assistant_favorites, "models", "root.assistant.favorites")?;
        }
    }

    Ok(())
}

pub fn favorite_voices_for_engine(engine: &str) -> Result<Vec<String>, String> {
    let cfg = effective_config_doc()?;
    let root = cfg
        .as_table()
        .ok_or_else(|| "Config root is not a TOML table".to_string())?;
    let tts = root
        .get("tts")
        .and_then(|v| v.as_table())
        .ok_or_else(|| "root.tts must be a table".to_string())?;
    let favorites = tts
        .get("favorites")
        .and_then(|v| v.as_table())
        .ok_or_else(|| "root.tts.favorites must be a table".to_string())?;

    let key = match engine {
        "pocket-tts" => "pocket_tts",
        "supertonic-3" => "supertonic_3",
        other => other,
    };

    if let Some(values) = favorites.get(key).and_then(|v| v.as_array()) {
        let mut out = Vec::with_capacity(values.len());
        for (idx, item) in values.iter().enumerate() {
            let Some(value) = item.as_str() else {
                return Err(format!("root.tts.favorites.{key}[{idx}] must be a string"));
            };
            if value.trim().is_empty() {
                return Err(format!("root.tts.favorites.{key}[{idx}] must be non-empty"));
            }
            out.push(value.to_string());
        }
        return Ok(out);
    }

    if key == "pocket_tts" {
        if let Some(values) = favorites.get("pocket-tts").and_then(|v| v.as_array()) {
            let mut out = Vec::with_capacity(values.len());
            for (idx, item) in values.iter().enumerate() {
                let Some(value) = item.as_str() else {
                    return Err(format!("root.tts.favorites.pocket-tts[{idx}] must be a string"));
                };
                if value.trim().is_empty() {
                    return Err(format!("root.tts.favorites.pocket-tts[{idx}] must be non-empty"));
                }
                out.push(value.to_string());
            }
            return Ok(out);
        }
    }

    if key == "supertonic_3" {
        if let Some(values) = favorites.get("supertonic-3").and_then(|v| v.as_array()) {
            let mut out = Vec::with_capacity(values.len());
            for (idx, item) in values.iter().enumerate() {
                let Some(value) = item.as_str() else {
                    return Err(format!("root.tts.favorites.supertonic-3[{idx}] must be a string"));
                };
                if value.trim().is_empty() {
                    return Err(format!("root.tts.favorites.supertonic-3[{idx}] must be non-empty"));
                }
                out.push(value.to_string());
            }
            return Ok(out);
        }
    }

    Ok(Vec::new())
}

pub fn favorite_models() -> Result<Vec<String>, String> {
    let cfg = effective_config_doc()?;
    let root = cfg
        .as_table()
        .ok_or_else(|| "Config root is not a TOML table".to_string())?;
    let assistant = root
        .get("assistant")
        .and_then(|v| v.as_table())
        .ok_or_else(|| "root.assistant must be a table".to_string())?;
    let favorites = assistant
        .get("favorites")
        .and_then(|v| v.as_table())
        .ok_or_else(|| "root.assistant.favorites must be a table".to_string())?;

    if let Some(values) = favorites.get("models").and_then(|v| v.as_array()) {
        let mut out = Vec::with_capacity(values.len());
        for (idx, item) in values.iter().enumerate() {
            let Some(value) = item.as_str() else {
                return Err(format!("root.assistant.favorites.models[{idx}] must be a string"));
            };
            if value.trim().is_empty() {
                return Err(format!("root.assistant.favorites.models[{idx}] must be non-empty"));
            }
            out.push(value.to_string());
        }
        return Ok(out);
    }

    Ok(Vec::new())
}

pub fn show() -> Result<(), String> {
    let cfg = effective_config_doc()?;
    validate_document(&cfg)?;
    let text = toml::to_string_pretty(&cfg).map_err(|e| format!("Failed to encode config TOML: {e}"))?;
    println!("{text}");
    Ok(())
}

pub fn validate() -> Result<(), String> {
    let cfg = effective_config_doc()?;
    validate_document(&cfg)?;
    let path = config_path()?;
    if path.exists() {
        println!("Config is valid: {}", path.display());
    } else {
        println!("Config is valid (using built-in defaults): {}", path.display());
    }
    Ok(())
}

pub fn show_path() -> Result<(), String> {
    let path = config_path()?;
    println!("{}", path.display());
    Ok(())
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
    if !matches!(engine, "kokoro" | "pocket-tts" | "supertonic-3") {
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
