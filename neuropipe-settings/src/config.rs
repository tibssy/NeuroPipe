use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use toml::{map::Map, Value};

pub const DEFAULT_ENGINE: &str = "kokoro";
pub const DEFAULT_VOICE: &str = "af_bella";
pub const DEFAULT_SPEED: f64 = 1.0;
pub const DEFAULT_QUALITY: &str = "high";
pub const DEFAULT_IDLE_TIMEOUT_SEC: u64 = 60;
pub const DEFAULT_ASSISTANT_DECISION_MODEL: &str = "none";
pub const DEFAULT_ASSISTANT_MODEL: &str = "llama3.2:1b";
pub const DEFAULT_ASSISTANT_SEARCH_FOLDERS: &[&str] = &[
    "~/Downloads",
    "~/Documents",
    "~/Videos",
    "~/Music",
    "~/Pictures",
];
pub const DEFAULT_ASSISTANT_SYSTEM_PROMPT: &str = "You are a helpful AI voice assistant.\nKeep answers short and conversational.\nThis is a voice-to-voice conversation: assume the user replies by speaking, not typing.\nIf you need confirmation (for example before using a tool in ask mode), request a spoken yes/no response and never ask the user to type.\n/set nothink";
pub const DEFAULT_ASSISTANT_TOOL_POLICY: &str = "When the user asks about something a tool can help with, call the appropriate tool automatically. If a tool is in ask mode, request spoken permission (yes/no) and continue based on the user's voice response. Do not ask the user to type permission commands.";

fn config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/neuropipe/config.toml")
}

fn load_doc() -> Value {
    let Ok(raw) = fs::read_to_string(config_path()) else {
        return Value::Table(Map::new());
    };
    raw.parse::<Value>().unwrap_or_else(|_| Value::Table(Map::new()))
}

/// Configured decision model preference. This is a settings placeholder only;
/// the assistant service does not use it yet.
pub fn assistant_decision_model() -> String {
    let doc = load_doc();
    doc
        .get("assistant")
        .and_then(|v| v.get("decision_model"))
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .unwrap_or(DEFAULT_ASSISTANT_DECISION_MODEL)
        .to_string()
}

/// Persist the decision-model placeholder selection under `[assistant]`.
pub fn persist_assistant_decision_model(model: &str) -> Result<(), String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("Assistant decision model must be non-empty".to_string());
    }
    let mut doc = load_doc();
    let root = doc.as_table_mut().ok_or("config root is not a table")?;
    let assistant = ensure_table(root, "assistant");
    assistant.insert(
        "decision_model".to_string(),
        Value::String(model.to_string()),
    );
    save_doc(&doc)
}

fn assistant_value(section: &str, key: &str) -> Option<Value> {
    let doc = load_doc();
    let assistant = doc.get("assistant")?;
    if section == "assistant" {
        assistant.get(key).cloned()
    } else {
        assistant.get(section)?.get(key).cloned()
    }
}

pub fn assistant_string(section: &str, key: &str, fallback: &str) -> String {
    assistant_value(section, key)
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| fallback.to_string())
}

pub fn assistant_bool(section: &str, key: &str, fallback: bool) -> bool {
    assistant_value(section, key)
        .and_then(|value| value.as_bool())
        .unwrap_or(fallback)
}

pub fn assistant_int(section: &str, key: &str, fallback: i64) -> i64 {
    assistant_value(section, key)
        .and_then(|value| value.as_integer())
        .unwrap_or(fallback)
}

pub fn assistant_float(section: &str, key: &str, fallback: f64) -> f64 {
    assistant_value(section, key)
        .and_then(|value| value.as_float().or_else(|| value.as_integer().map(|n| n as f64)))
        .unwrap_or(fallback)
}

pub fn assistant_default_model() -> String {
    assistant_string("assistant", "default_model", DEFAULT_ASSISTANT_MODEL)
}

pub fn assistant_favorite_models() -> Vec<String> {
    assistant_value("favorites", "models")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect()
}

fn persist_assistant_model_favorites(models: Vec<String>) -> Result<(), String> {
    let mut doc = load_doc();
    let root = doc.as_table_mut().ok_or("config root is not a table")?;
    let assistant = ensure_table(root, "assistant");
    let favorites = ensure_table(assistant, "favorites");
    favorites.insert(
        "models".to_string(),
        Value::Array(models.into_iter().map(Value::String).collect()),
    );
    save_doc(&doc)
}

pub fn persist_assistant_favorite_add(model: &str) -> Result<(), String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("Assistant model must be non-empty".to_string());
    }
    let mut models = assistant_favorite_models();
    if !models.iter().any(|existing| existing == model) {
        models.push(model.to_string());
    }
    persist_assistant_model_favorites(models)
}

pub fn persist_assistant_favorite_remove(model: &str) -> Result<(), String> {
    let models = assistant_favorite_models()
        .into_iter()
        .filter(|existing| existing != model)
        .collect();
    persist_assistant_model_favorites(models)
}

pub fn assistant_search_folders() -> Vec<String> {
    match assistant_value("assistant", "search_folders") {
        Some(Value::Array(values)) => values
            .into_iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect(),
        _ => DEFAULT_ASSISTANT_SEARCH_FOLDERS
            .iter()
            .map(|folder| (*folder).to_string())
            .collect(),
    }
}

fn persist_assistant_search_folders(folders: Vec<String>) -> Result<(), String> {
    let mut doc = load_doc();
    let root = doc.as_table_mut().ok_or("config root is not a table")?;
    let assistant = ensure_table(root, "assistant");
    assistant.insert(
        "search_folders".to_string(),
        Value::Array(folders.into_iter().map(Value::String).collect()),
    );
    save_doc(&doc)
}

pub fn persist_assistant_search_folder_add(folder: &str) -> Result<(), String> {
    let folder = folder.trim();
    if folder.is_empty() {
        return Err("Search folder must be non-empty".to_string());
    }
    let mut folders = assistant_search_folders();
    if !folders.iter().any(|existing| existing == folder) {
        folders.push(folder.to_string());
    }
    persist_assistant_search_folders(folders)
}

pub fn persist_assistant_search_folder_remove(folder: &str) -> Result<(), String> {
    let folders = assistant_search_folders()
        .into_iter()
        .filter(|existing| existing != folder)
        .collect();
    persist_assistant_search_folders(folders)
}

pub fn assistant_tool_permissions() -> Vec<(String, String)> {
    let defaults = [
        ("open_url", "ask"),
        ("screenshot", "ask"),
        ("web_search", "ask"),
        ("media_control", "allow"),
        ("clipboard", "ask"),
        ("file_search", "ask"),
    ];
    let mut permissions: std::collections::BTreeMap<String, String> = defaults
        .iter()
        .map(|(name, permission)| ((*name).to_string(), (*permission).to_string()))
        .collect();
    if let Some(Value::Table(configured)) = assistant_value("assistant", "tools") {
        for (name, value) in configured {
            if let Some(permission) = value.as_str() {
                permissions.insert(name, permission.to_string());
            }
        }
    }
    permissions.into_iter().collect()
}

/// Persist an `[assistant]` scalar or a value in one of its nested tables.
pub fn persist_assistant_value(section: &str, key: &str, value: Value) -> Result<(), String> {
    if key.trim().is_empty() {
        return Err("Assistant setting key must be non-empty".to_string());
    }
    if !matches!(section, "assistant" | "memory" | "instructions" | "tools") {
        return Err(format!("Unsupported assistant config section '{section}'"));
    }
    let mut doc = load_doc();
    let root = doc.as_table_mut().ok_or("config root is not a table")?;
    let assistant = ensure_table(root, "assistant");
    let target = if section == "assistant" {
        assistant
    } else {
        ensure_table(assistant, section)
    };
    target.insert(key.to_string(), value);
    save_doc(&doc)
}

pub fn persist_assistant_instructions(system_prompt: &str, tool_policy: &str) -> Result<(), String> {
    if system_prompt.trim().is_empty() {
        return Err("Assistant system prompt must be non-empty".to_string());
    }
    if tool_policy.trim().is_empty() {
        return Err("Assistant tool usage policy must be non-empty".to_string());
    }
    let mut doc = load_doc();
    let root = doc.as_table_mut().ok_or("config root is not a table")?;
    let assistant = ensure_table(root, "assistant");
    let instructions = ensure_table(assistant, "instructions");
    instructions.insert(
        "system_prompt".to_string(),
        Value::String(system_prompt.to_string()),
    );
    instructions.insert(
        "tool_usage_policy".to_string(),
        Value::String(tool_policy.to_string()),
    );
    save_doc(&doc)
}

fn ensure_table<'a>(map: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    if !matches!(map.get(key), Some(Value::Table(_))) {
        map.insert(key.to_string(), Value::Table(Map::new()));
    }
    map.get_mut(key)
        .and_then(Value::as_table_mut)
        .expect("table just inserted")
}

/// Current `[tts.defaults]` engine + voice from config, with sane defaults.
pub fn tts_defaults() -> (String, String) {
    let doc = load_doc();
    tts_defaults_from(&doc)
}

fn tts_defaults_from(doc: &Value) -> (String, String) {
    let engine = doc
        .get("tts")
        .and_then(|v| v.get("defaults"))
        .and_then(|v| v.get("engine"))
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_ENGINE)
        .to_string();
    let voice = doc
        .get("tts")
        .and_then(|v| v.get("defaults"))
        .and_then(|v| v.get("voice"))
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_VOICE)
        .to_string();
    (engine, voice)
}

fn defaults_table(doc: &Value) -> Option<&Map<String, Value>> {
    doc.get("tts")?
        .get("defaults")?
        .as_table()
}

/// Effective speed for `engine`: `[tts.speeds]` override, else the
/// `[tts.defaults].speed` fallback. Same resolution the service uses.
pub fn tts_speed_for(engine: &str) -> f64 {
    let doc = load_doc();
    tts_speed_from(&doc, engine)
}

fn tts_speed_from(doc: &Value, engine: &str) -> f64 {
    let fallback = defaults_table(doc)
        .and_then(|t| t.get("speed"))
        .and_then(Value::as_float)
        .unwrap_or(DEFAULT_SPEED);
    doc.get("tts")
        .and_then(|v| v.get("speeds"))
        .and_then(|v| v.get(engine))
        .and_then(Value::as_float)
        .unwrap_or(fallback)
}

/// Effective quality for `engine`: `[tts.qualities]` override, else the
/// `[tts.defaults].quality` fallback. Same resolution the service uses.
pub fn tts_quality_for(engine: &str) -> String {
    let doc = load_doc();
    tts_quality_from(&doc, engine)
}

fn tts_quality_from(doc: &Value, engine: &str) -> String {
    let fallback = defaults_table(doc)
        .and_then(|t| t.get("quality"))
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_QUALITY);
    doc.get("tts")
        .and_then(|v| v.get("qualities"))
        .and_then(|v| v.get(engine))
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_string()
}

/// `[tts.defaults].idle_timeout_sec`, defaulting to 60.
pub fn tts_idle_timeout_sec() -> u64 {
    let doc = load_doc();
    defaults_table(&doc)
        .and_then(|t| t.get("idle_timeout_sec"))
        .and_then(Value::as_integer)
        .unwrap_or(DEFAULT_IDLE_TIMEOUT_SEC as i64)
        .max(1) as u64
}

/// Config key for an engine's favorite voices. Matches the CLI's convention
/// (`pocket-tts` -> `pocket_tts`, `supertonic-3` -> `supertonic_3`), which the
/// service cycle commands read from.
fn favorites_key(engine: &str) -> String {
    engine.replace('-', "_")
}

/// Favorite voices for `engine` from `[tts.favorites]` (empty when unset).
pub fn tts_favorite_voices(engine: &str) -> Vec<String> {
    let doc = load_doc();
    let key = favorites_key(engine);
    doc.get("tts")
        .and_then(|v| v.get("favorites"))
        .and_then(|v| v.get(&key))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Persist `voice` as a favorite for `engine`, appending if not already present.
pub fn persist_tts_favorite_add(engine: &str, voice: &str) -> Result<(), String> {
    if engine.is_empty() || voice.is_empty() {
        return Err("engine and voice must be non-empty".to_string());
    }
    let mut doc = load_doc();
    let root = doc.as_table_mut().ok_or("config root is not a table")?;
    let tts = ensure_table(root, "tts");
    let favorites = ensure_table(tts, "favorites");
    let key = favorites_key(engine);
    let mut list = favorites
        .get(&key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if list.iter().all(|v| v.as_str() != Some(voice)) {
        list.push(Value::String(voice.to_string()));
    }
    favorites.insert(key, Value::Array(list));
    save_doc(&doc)
}

/// Remove `voice` from `engine`'s favorites.
pub fn persist_tts_favorite_remove(engine: &str, voice: &str) -> Result<(), String> {
    if engine.is_empty() || voice.is_empty() {
        return Err("engine and voice must be non-empty".to_string());
    }
    let mut doc = load_doc();
    let root = doc.as_table_mut().ok_or("config root is not a table")?;
    let tts = ensure_table(root, "tts");
    let favorites = ensure_table(tts, "favorites");
    let key = favorites_key(engine);
    let mut list = favorites
        .get(&key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    list.retain(|v| v.as_str() != Some(voice));
    favorites.insert(key, Value::Array(list));
    save_doc(&doc)
}

fn write_atomic(path: &PathBuf, content: &str) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent dir"))?;
    fs::create_dir_all(parent)?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = path.with_extension(format!("tmp.{}.{}", std::process::id(), ts));
    let mut f = fs::File::create(&tmp)?;
    f.write_all(content.as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    let dir = fs::File::open(parent)?;
    dir.sync_all()?;
    Ok(())
}

/// Persist `[tts.defaults].engine` and `[tts.defaults].voice`, preserving the
/// rest of the config file.
pub fn persist_tts_defaults(engine: &str, voice: &str) -> Result<(), String> {
    if engine.is_empty() || voice.is_empty() {
        return Err("engine and voice must be non-empty".to_string());
    }
    let mut doc = load_doc();
    let root = doc.as_table_mut().ok_or("config root is not a table")?;
    let tts = ensure_table(root, "tts");
    let defaults = ensure_table(tts, "defaults");
    defaults.insert("engine".to_string(), Value::String(engine.to_string()));
    defaults.insert("voice".to_string(), Value::String(voice.to_string()));
    save_doc(&doc)
}

/// Persist a per-engine speed override into `[tts.speeds]`.
pub fn persist_tts_speed(engine: &str, speed: f64) -> Result<(), String> {
    if !(0.5..=2.0).contains(&speed) {
        return Err(format!("Invalid speed '{speed}'. Must be in [0.5, 2.0]."));
    }
    let mut doc = load_doc();
    let root = doc.as_table_mut().ok_or("config root is not a table")?;
    let tts = ensure_table(root, "tts");
    let speeds = ensure_table(tts, "speeds");
    speeds.insert(engine.to_string(), Value::Float(speed));
    save_doc(&doc)
}

/// Persist a per-engine quality override into `[tts.qualities]`.
pub fn persist_tts_quality(engine: &str, quality: &str) -> Result<(), String> {
    if !matches!(quality, "low" | "high") {
        return Err(format!("Invalid quality '{quality}'."));
    }
    let mut doc = load_doc();
    let root = doc.as_table_mut().ok_or("config root is not a table")?;
    let tts = ensure_table(root, "tts");
    let qualities = ensure_table(tts, "qualities");
    qualities.insert(engine.to_string(), Value::String(quality.to_string()));
    save_doc(&doc)
}

/// Persist `[tts.defaults].idle_timeout_sec`.
pub fn persist_tts_idle_timeout(idle_timeout_sec: u64) -> Result<(), String> {
    if idle_timeout_sec < 1 {
        return Err("Idle timeout must be >= 1 second.".to_string());
    }
    let mut doc = load_doc();
    let root = doc.as_table_mut().ok_or("config root is not a table")?;
    let tts = ensure_table(root, "tts");
    let defaults = ensure_table(tts, "defaults");
    defaults.insert(
        "idle_timeout_sec".to_string(),
        Value::Integer(idle_timeout_sec as i64),
    );
    save_doc(&doc)
}

/// STT defaults, mirroring the STT service's own defaults so a fresh config
/// renders the same values the service actually uses.
pub const DEFAULT_STT_MODEL: &str = "nemo-parakeet-tdt-0.6b-v3";
pub const DEFAULT_STT_MODEL_DIR: &str = "~/.local/share/neuropipe/stt/parakeet-v3";
pub const DEFAULT_STT_REDUX_MODEL: &str = "nemo-parakeet-tdt-0.6b-v3-redux";
pub const DEFAULT_STT_REDUX_MODEL_DIR: &str = "~/.local/share/neuropipe/stt/parakeet-redux";
/// Bundled ASR models: (label, `[stt].model`, `[stt].model_dir`).
pub const STT_MODEL_PRESETS: &[(&str, &str, &str)] = &[
    ("parakeet-v3", DEFAULT_STT_MODEL, DEFAULT_STT_MODEL_DIR),
    (
        "parakeet-redux",
        DEFAULT_STT_REDUX_MODEL,
        DEFAULT_STT_REDUX_MODEL_DIR,
    ),
];
pub const DEFAULT_STT_VAD_THRESHOLD: f64 = 0.5;
pub const DEFAULT_STT_VAD_START_FRAMES: i64 = 5;
pub const DEFAULT_STT_DIGITAL_GAIN: f64 = 3.0;
pub const DEFAULT_STT_SILENCE_TIMEOUT_SEC: f64 = 1.0;
pub const DEFAULT_STT_MODEL_IDLE_TIMEOUT_SEC: i64 = 60;
pub const DEFAULT_STT_TURN_END_ENABLED: bool = true;
pub const DEFAULT_STT_TURN_HOLD_MS: i64 = 900;
pub const DEFAULT_STT_TURN_END_THRESHOLD: f64 = 0.5;
pub const DEFAULT_STT_TURN_SCORE_CADENCE_MS: i64 = 500;
pub const DEFAULT_STT_TURN_HARD_CEILING_MS: i64 = 3500;
pub const DEFAULT_STT_TURN_DETECTOR: &str = "smart_turn";
pub const DEFAULT_STT_SMART_TURN_MODEL_PATH: &str =
    "~/.local/share/neuropipe/stt/smart_turn_v3.2_cpu.onnx";
pub const DEFAULT_STT_SMART_TURN_MIN_UTTERANCE_MS: i64 = 1200;

fn stt_table(doc: &Value) -> Option<&Map<String, Value>> {
    doc.get("stt")?.as_table()
}

/// `[stt]` string value, empty when unset.
pub fn stt_string(key: &str) -> String {
    let doc = load_doc();
    stt_table(&doc)
        .and_then(|t| t.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `[stt]` float value, `fallback` when unset.
pub fn stt_float(key: &str, fallback: f64) -> f64 {
    let doc = load_doc();
    stt_table(&doc)
        .and_then(|t| t.get(key))
        .and_then(Value::as_float)
        .unwrap_or(fallback)
}

/// `[stt]` integer value, `fallback` when unset.
pub fn stt_int(key: &str, fallback: i64) -> i64 {
    let doc = load_doc();
    stt_table(&doc)
        .and_then(|t| t.get(key))
        .and_then(Value::as_integer)
        .unwrap_or(fallback)
}

/// `[stt]` boolean value, `fallback` when unset.
pub fn stt_bool(key: &str, fallback: bool) -> bool {
    let doc = load_doc();
    stt_table(&doc)
        .and_then(|t| t.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(fallback)
}

/// Persist one `[stt]` scalar (string, int, float or bool), preserving the
/// rest of the config file.
pub fn persist_stt_scalar(key: &str, value: Value) -> Result<(), String> {
    if key.is_empty() {
        return Err("STT key must be non-empty".to_string());
    }
    let mut doc = load_doc();
    let root = doc.as_table_mut().ok_or("config root is not a table")?;
    let stt = ensure_table(root, "stt");
    stt.insert(key.to_string(), value);
    save_doc(&doc)
}

/// Persist the `[stt]` model and its directory in a single write, so the
/// service's model metadata and the engine path it loads never diverge.
pub fn persist_stt_model(model: &str, model_dir: &str) -> Result<(), String> {
    if model.trim().is_empty() {
        return Err("STT model must be non-empty".to_string());
    }
    if model_dir.trim().is_empty() {
        return Err("STT model directory must be non-empty".to_string());
    }
    let mut doc = load_doc();
    let root = doc.as_table_mut().ok_or("config root is not a table")?;
    let stt = ensure_table(root, "stt");
    stt.insert("model".to_string(), Value::String(model.to_string()));
    stt.insert("model_dir".to_string(), Value::String(model_dir.to_string()));
    save_doc(&doc)
}

fn save_doc(doc: &Value) -> Result<(), String> {
    let text = toml::to_string_pretty(doc).map_err(|e| format!("encode config: {e}"))?;
    write_atomic(&config_path(), &text).map_err(|e| format!("write config: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `config_path()` reads the process-wide HOME env var, so tests that swap
    /// it must run one at a time.
    static HOME_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn defaults_fall_back_when_missing() {
        let doc = Value::Table(Map::new());
        let (engine, voice) = tts_defaults_from(&doc);
        assert_eq!(engine, DEFAULT_ENGINE);
        assert_eq!(voice, DEFAULT_VOICE);
    }

    #[test]
    fn defaults_read_from_doc() {
        let doc = "[tts.defaults]\nengine = \"supertonic-3\"\nvoice = \"M3\"\n"
            .parse::<Value>()
            .unwrap();
        let (engine, voice) = tts_defaults_from(&doc);
        assert_eq!(engine, "supertonic-3");
        assert_eq!(voice, "M3");
    }

    #[test]
    fn speed_resolves_per_engine_then_fallback() {
        let doc = "[tts.defaults]\nspeed = 1.0\n\n[tts.speeds]\nkokoro = 1.1\n"
            .parse::<Value>()
            .unwrap();

        // Per-engine entry wins; otherwise the defaults.speed fallback applies.
        assert_eq!(tts_speed_from(&doc, "kokoro"), 1.1);
        assert_eq!(tts_speed_from(&doc, "pocket-tts"), 1.0);
    }

    #[test]
    fn quality_resolves_per_engine_then_fallback() {
        let doc = "[tts.defaults]\nquality = \"high\"\n\n[tts.qualities]\nkokoro = \"low\"\n"
            .parse::<Value>()
            .unwrap();

        // Per-engine entry wins; otherwise the defaults.quality fallback applies.
        assert_eq!(tts_quality_from(&doc, "kokoro"), "low");
        assert_eq!(tts_quality_from(&doc, "pocket-tts"), "high");
    }

    #[test]
    fn persist_roundtrip() {
        let _guard = HOME_LOCK.lock().unwrap();
        // Isolate from the real config file, restoring HOME afterwards.
        let original_home = std::env::var("HOME").ok();
        let dir = std::env::temp_dir().join(format!("np_settings_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".config/neuropipe")).unwrap();
        std::env::set_var("HOME", &dir);
        fs::write(
            dir.join(".config/neuropipe/config.toml"),
            "[tts.defaults]\nengine = \"kokoro\"\nvoice = \"af_bella\"\nquality = \"high\"\nidle_timeout_sec = 60\nspeed = 1.0\n",
        )
        .unwrap();

        persist_tts_speed("kokoro", 1.25).unwrap();
        persist_tts_quality("kokoro", "low").unwrap();
        persist_tts_idle_timeout(120).unwrap();

        assert_eq!(tts_speed_for("kokoro"), 1.25);
        assert_eq!(tts_quality_for("kokoro"), "low");
        assert_eq!(tts_quality_for("pocket-tts"), "high");
        assert_eq!(tts_idle_timeout_sec(), 120);

        // Other fields survive the writes.
        let (engine, voice) = tts_defaults();
        assert_eq!(engine, "kokoro");
        assert_eq!(voice, "af_bella");

        let _ = fs::remove_dir_all(&dir);
        match original_home {
            Some(home) => std::env::set_var("HOME", home),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn assistant_decision_model_roundtrip() {
        let _guard = HOME_LOCK.lock().unwrap();
        let original_home = std::env::var("HOME").ok();
        let dir = std::env::temp_dir().join(format!(
            "np_settings_assistant_decision_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".config/neuropipe")).unwrap();
        std::env::set_var("HOME", &dir);
        fs::write(
            dir.join(".config/neuropipe/config.toml"),
            "[assistant]\ndefault_model = \"llama3.2:1b\"\n",
        )
        .unwrap();

        assert_eq!(assistant_decision_model(), "none");
        persist_assistant_decision_model("laya").unwrap();
        assert_eq!(assistant_decision_model(), "laya");
        persist_assistant_decision_model("tev1:0.8b").unwrap();
        assert_eq!(assistant_decision_model(), "tev1:0.8b");
        persist_assistant_decision_model("none").unwrap();
        assert_eq!(assistant_decision_model(), "none");
        assert!(persist_assistant_decision_model("  ").is_err());

        // The existing assistant configuration survives the placeholder write.
        let doc = load_doc();
        assert_eq!(
            doc.get("assistant")
                .and_then(|v| v.get("default_model"))
                .and_then(Value::as_str),
            Some("llama3.2:1b")
        );

        assert_eq!(assistant_default_model(), "llama3.2:1b");
        assert_eq!(assistant_favorite_models(), Vec::<String>::new());
        persist_assistant_favorite_add("qwen3:4b").unwrap();
        persist_assistant_favorite_add("qwen3:4b").unwrap();
        assert_eq!(assistant_favorite_models(), vec!["qwen3:4b".to_string()]);
        persist_assistant_favorite_remove("qwen3:4b").unwrap();
        assert!(assistant_favorite_models().is_empty());

        let default_folders = assistant_search_folders();
        assert_eq!(default_folders.len(), DEFAULT_ASSISTANT_SEARCH_FOLDERS.len());
        persist_assistant_search_folder_add("/tmp/np-search-test").unwrap();
        assert!(assistant_search_folders().contains(&"/tmp/np-search-test".to_string()));
        persist_assistant_search_folder_remove("/tmp/np-search-test").unwrap();
        assert_eq!(assistant_search_folders(), default_folders);
        for folder in default_folders {
            persist_assistant_search_folder_remove(&folder).unwrap();
        }
        assert!(assistant_search_folders().is_empty());
        assert_eq!(assistant_tool_permissions().len(), 6);

        let _ = fs::remove_dir_all(&dir);
        match original_home {
            Some(home) => std::env::set_var("HOME", home),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn stt_getters_fall_back_when_missing() {
        // These getters read config.toml from $HOME, so isolate HOME and take
        // the same lock as the roundtrip test instead of racing it.
        let _guard = HOME_LOCK.lock().unwrap();
        let original_home = std::env::var("HOME").ok();
        let dir = std::env::temp_dir().join(format!("np_settings_stt_empty_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        std::env::set_var("HOME", &dir);

        let doc = Value::Table(Map::new());
        assert_eq!(stt_table(&doc), None);
        assert!(stt_string("model").is_empty());
        assert_eq!(stt_float("vad_threshold", DEFAULT_STT_VAD_THRESHOLD), 0.5);
        assert_eq!(stt_int("vad_start_frames", DEFAULT_STT_VAD_START_FRAMES), 5);
        assert!(stt_bool("turn_end_enabled", DEFAULT_STT_TURN_END_ENABLED));

        let _ = fs::remove_dir_all(&dir);
        match original_home {
            Some(home) => std::env::set_var("HOME", home),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn stt_persist_scalar_roundtrip() {
        let _guard = HOME_LOCK.lock().unwrap();
        let original_home = std::env::var("HOME").ok();
        let dir = std::env::temp_dir().join(format!("np_settings_stt_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".config/neuropipe")).unwrap();
        std::env::set_var("HOME", &dir);
        fs::write(
            dir.join(".config/neuropipe/config.toml"),
            "[tts.defaults]\nengine = \"kokoro\"\nvoice = \"af_bella\"\nquality = \"high\"\nidle_timeout_sec = 60\nspeed = 1.0\n\n[stt]\nvad_threshold = 0.4\n",
        )
        .unwrap();

        assert_eq!(stt_float("vad_threshold", DEFAULT_STT_VAD_THRESHOLD), 0.4);
        persist_stt_scalar("vad_threshold", Value::Float(0.55)).unwrap();
        persist_stt_scalar("vad_start_frames", Value::Integer(7)).unwrap();
        persist_stt_scalar("digital_gain", Value::Float(2.5)).unwrap();
        persist_stt_scalar("turn_end_enabled", Value::Boolean(false)).unwrap();
        persist_stt_scalar("model", Value::String("my-model".to_string())).unwrap();

        assert_eq!(stt_float("vad_threshold", DEFAULT_STT_VAD_THRESHOLD), 0.55);
        assert_eq!(stt_int("vad_start_frames", DEFAULT_STT_VAD_START_FRAMES), 7);
        assert_eq!(stt_float("digital_gain", DEFAULT_STT_DIGITAL_GAIN), 2.5);
        assert!(!stt_bool("turn_end_enabled", DEFAULT_STT_TURN_END_ENABLED));
        assert_eq!(stt_string("model"), "my-model");

        // Model presets persist the id and directory in one write.
        persist_stt_model(DEFAULT_STT_REDUX_MODEL, DEFAULT_STT_REDUX_MODEL_DIR).unwrap();
        assert_eq!(stt_string("model"), DEFAULT_STT_REDUX_MODEL);
        assert_eq!(stt_string("model_dir"), DEFAULT_STT_REDUX_MODEL_DIR);

        // Unstored keys still fall back, and unrelated tables survive.
        assert_eq!(stt_float("turn_end_threshold", DEFAULT_STT_TURN_END_THRESHOLD), 0.5);
        let (engine, voice) = tts_defaults();
        assert_eq!(engine, "kokoro");
        assert_eq!(voice, "af_bella");

        let _ = fs::remove_dir_all(&dir);
        match original_home {
            Some(home) => std::env::set_var("HOME", home),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn stt_persist_rejects_empty_key() {
        assert!(persist_stt_scalar("", Value::Integer(1)).is_err());
    }

    #[test]
    fn favorites_roundtrip_with_engine_key_normalization() {
        let _guard = HOME_LOCK.lock().unwrap();
        let original_home = std::env::var("HOME").ok();
        let dir = std::env::temp_dir().join(format!("np_settings_fav_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".config/neuropipe")).unwrap();
        std::env::set_var("HOME", &dir);
        fs::write(
            dir.join(".config/neuropipe/config.toml"),
            "[tts.defaults]\nengine = \"kokoro\"\nvoice = \"af_bella\"\nquality = \"high\"\nidle_timeout_sec = 60\nspeed = 1.0\n",
        )
        .unwrap();

        assert!(tts_favorite_voices("kokoro").is_empty());
        persist_tts_favorite_add("kokoro", "af_bella").unwrap();
        persist_tts_favorite_add("kokoro", "af_heart").unwrap();
        persist_tts_favorite_add("kokoro", "af_bella").unwrap();
        persist_tts_favorite_add("pocket-tts", "v2/en_SZ").unwrap();

        let kokoro = tts_favorite_voices("kokoro");
        assert_eq!(kokoro, ["af_bella".to_string(), "af_heart".to_string()]);
        // Kebab-case engine is stored under the snake_case key the CLI reads.
        assert_eq!(tts_favorite_voices("pocket-tts"), ["v2/en_SZ".to_string()]);
        assert!(tts_favorite_voices("supertonic-3").is_empty());

        persist_tts_favorite_remove("kokoro", "af_bella").unwrap();
        assert_eq!(tts_favorite_voices("kokoro"), ["af_heart".to_string()]);
        persist_tts_favorite_remove("kokoro", "af_heart").unwrap();
        assert!(tts_favorite_voices("kokoro").is_empty());

        let _ = fs::remove_dir_all(&dir);
        match original_home {
            Some(home) => std::env::set_var("HOME", home),
            None => std::env::remove_var("HOME"),
        }
    }
}
