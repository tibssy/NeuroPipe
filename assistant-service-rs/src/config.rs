use std::{env, fs, path::PathBuf};

use toml::{map::Map as TomlMap, Value as TomlValue};

#[derive(Clone, Debug)]
pub struct MemoryConfig {
    pub enabled_local: bool,
    pub enabled_cloud: bool,
    pub summarize_on_idle: bool,
    pub summarize_on_stop: bool,
    pub max_summary_chars: usize,
    pub retrieve_top_k: usize,
    pub collection: String,
    pub embedding_model: String,
}

#[derive(Clone, Debug)]
pub struct AssistantConfig {
    pub default_model: String,
    pub history_idle_timeout_sec: u64,
    pub memory: MemoryConfig,
    pub system_prompt: String,
    pub tool_usage_policy: String,
    pub tools: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct IpcConfig {
    pub stt_cmd: String,
    pub stt_pub: String,
    pub tts_cmd: String,
    pub tts_events: String,
    pub assistant_cmd: String,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub ipc: IpcConfig,
    pub assistant: AssistantConfig,
}

const DEFAULT_CONFIG: &str = r#"
version = 1

[ipc]
stt_cmd = "ipc:///tmp/neuropipe_cmd.sock"
stt_pub = "ipc:///tmp/neuropipe_pub.sock"
tts_cmd = "ipc:///tmp/neuropipe_tts_cmd.sock"
tts_events = "ipc:///tmp/neuropipe_tts_events.sock"
assistant_cmd = "ipc:///tmp/neuropipe_assistant_cmd.sock"

[assistant]
default_model = "llama3.2:1b"
history_idle_timeout_sec = 3600

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
system_prompt = ""
tool_usage_policy = ""

[assistant.tools]
open_url = "ask"
screenshot = "ask"
web_search = "ask"
media_control = "allow"
"#;

fn config_path() -> PathBuf {
    let home = env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/neuropipe/config.toml")
}

fn parse_default() -> TomlValue {
    DEFAULT_CONFIG.parse::<TomlValue>().expect("default config parse")
}

fn merge_toml(defaults: &TomlValue, incoming: &TomlValue) -> TomlValue {
    match (defaults, incoming) {
        (TomlValue::Table(d), TomlValue::Table(i)) => {
            let mut m = d.clone();
            for (k, v) in i {
                m.insert(k.clone(), match m.get(k) {
                    Some(existing) => merge_toml(existing, v),
                    None => v.clone(),
                });
            }
            TomlValue::Table(m)
        }
        (_, v) => v.clone(),
    }
}

fn table<'a>(parent: &'a TomlMap<String, TomlValue>, key: &str) -> TomlMap<String, TomlValue> {
    parent.get(key).and_then(|v| v.as_table()).cloned().unwrap_or_default()
}

fn s<'a>(t: &'a TomlMap<String, TomlValue>, key: &str, default: &str) -> String {
    t.get(key).and_then(|v| v.as_str()).unwrap_or(default).to_string()
}

fn b(t: &TomlMap<String, TomlValue>, key: &str, default: bool) -> bool {
    t.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

fn i(t: &TomlMap<String, TomlValue>, key: &str, default: i64) -> i64 {
    t.get(key).and_then(|v| v.as_integer()).unwrap_or(default)
}

impl Config {
    pub fn load() -> Config {
        load_config()
    }
}

fn load_config() -> Config {
    let defaults = parse_default();
    let merged = match fs::read_to_string(config_path()) {
        Ok(text) => match text.parse::<TomlValue>() {
            Ok(raw) => merge_toml(&defaults, &raw),
            Err(e) => {
                eprintln!("[config] failed to parse {}: {e}", config_path().display());
                defaults.clone()
            }
        },
        Err(_) => defaults.clone(),
    };

    let root = merged.clone().as_table().cloned().unwrap_or_default();
    let ipc = table(&root, "ipc");
    let ast = table(&root, "assistant");

    let memory_t = table(&ast, "memory");
    let inst_t = table(&ast, "instructions");

    let sys_default = "You are a helpful AI voice assistant.\nKeep answers short and conversational.\nThis is a voice-to-voice conversation: assume the user replies by speaking, not typing.\nIf you need confirmation (for example before using a tool in ask mode), request a spoken yes/no response and never ask the user to type.\n/set nothink";
    let tools_default = "When the user asks about something a tool can help with, call the appropriate tool automatically. If a tool is in ask mode, request spoken permission (yes/no) and continue based on the user's voice response. Do not ask the user to type permission commands.";

    let mut tools = Vec::new();
    for (name, lvl) in table(&ast, "tools") {
        if let Some(l) = lvl.as_str() {
            tools.push((name.clone(), l.to_string()));
        }
    }

    let assistant = AssistantConfig {
        default_model: s(&ast, "default_model", "llama3.2:1b"),
        history_idle_timeout_sec: i(&ast, "history_idle_timeout_sec", 3600).max(1) as u64,
        memory: MemoryConfig {
            enabled_local: b(&memory_t, "enabled_local", true),
            enabled_cloud: b(&memory_t, "enabled_cloud", false),
            summarize_on_idle: b(&memory_t, "summarize_on_idle", true),
            summarize_on_stop: b(&memory_t, "summarize_on_stop", true),
            max_summary_chars: i(&memory_t, "max_summary_chars", 1200).max(0) as usize,
            retrieve_top_k: i(&memory_t, "retrieve_top_k", 4).max(1) as usize,
            collection: s(&memory_t, "collection", "assistant_memory"),
            embedding_model: s(&memory_t, "embedding_model", "all-minilm"),
        },
        system_prompt: s(&inst_t, "system_prompt", sys_default),
        tool_usage_policy: s(&inst_t, "tool_usage_policy", tools_default),
        tools,
    };

    Config {
        ipc: IpcConfig {
            stt_cmd: s(&ipc, "stt_cmd", "ipc:///tmp/neuropipe_cmd.sock"),
            stt_pub: s(&ipc, "stt_pub", "ipc:///tmp/neuropipe_pub.sock"),
            tts_cmd: s(&ipc, "tts_cmd", "ipc:///tmp/neuropipe_tts_cmd.sock"),
            tts_events: s(&ipc, "tts_events", "ipc:///tmp/neuropipe_tts_events.sock"),
            assistant_cmd: s(&ipc, "assistant_cmd", "ipc:///tmp/neuropipe_assistant_cmd.sock"),
        },
        assistant,
    }
}