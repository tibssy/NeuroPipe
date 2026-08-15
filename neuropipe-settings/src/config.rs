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
