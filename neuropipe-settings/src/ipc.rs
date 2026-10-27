use std::thread;
use std::time::Duration;
use std::{fs, path::PathBuf};

use futures_channel::mpsc;
use serde_json::{json, Value};

/// Default TTS command socket, matching the services' fallback config.
const DEFAULT_TTS_CMD: &str = "ipc:///tmp/neuropipe_tts_cmd.sock";

/// Default STT command socket, matching the services' fallback config.
const DEFAULT_STT_CMD: &str = "ipc:///tmp/neuropipe_cmd.sock";
const DEFAULT_ASSISTANT_CMD: &str = "ipc:///tmp/neuropipe_assistant_cmd.sock";

fn config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/neuropipe/config.toml")
}

/// TTS command socket from `[ipc].tts_cmd`, falling back to the default the
/// services ship with.
fn tts_cmd_addr() -> String {
    let Ok(raw) = fs::read_to_string(config_path()) else {
        return DEFAULT_TTS_CMD.to_string();
    };
    let Ok(parsed) = raw.parse::<toml::Value>() else {
        return DEFAULT_TTS_CMD.to_string();
    };
    parsed
        .get("ipc")
        .and_then(|v| v.get("tts_cmd"))
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_TTS_CMD)
        .to_string()
}

/// STT command socket from `[ipc].stt_cmd`, falling back to the default the
/// services ship with.
fn stt_cmd_addr() -> String {
    let Ok(raw) = fs::read_to_string(config_path()) else {
        return DEFAULT_STT_CMD.to_string();
    };
    let Ok(parsed) = raw.parse::<toml::Value>() else {
        return DEFAULT_STT_CMD.to_string();
    };
    parsed
        .get("ipc")
        .and_then(|v| v.get("stt_cmd"))
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_STT_CMD)
        .to_string()
}

fn assistant_cmd_addr() -> String {
    let Ok(raw) = fs::read_to_string(config_path()) else {
        return DEFAULT_ASSISTANT_CMD.to_string();
    };
    let Ok(parsed) = raw.parse::<toml::Value>() else {
        return DEFAULT_ASSISTANT_CMD.to_string();
    };
    parsed
        .get("ipc")
        .and_then(|v| v.get("assistant_cmd"))
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_ASSISTANT_CMD)
        .to_string()
}

fn send_assistant_cmd(cmd: &Value) -> Result<Value, String> {
    send_assistant_cmd_with_timeout(cmd, 1500)
}

fn send_assistant_cmd_with_timeout(cmd: &Value, timeout_ms: i32) -> Result<Value, String> {
    let ctx = zmq::Context::new();
    let socket = ctx.socket(zmq::REQ).map_err(|e| e.to_string())?;
    socket.set_linger(0).map_err(|e| e.to_string())?;
    socket.set_sndtimeo(timeout_ms).map_err(|e| e.to_string())?;
    socket.set_rcvtimeo(timeout_ms).map_err(|e| e.to_string())?;
    socket
        .connect(&assistant_cmd_addr())
        .map_err(|e| e.to_string())?;
    let payload = serde_json::to_vec(cmd).map_err(|e| e.to_string())?;
    socket.send(&payload, 0).map_err(|e| e.to_string())?;
    let reply = socket.recv_bytes(0).map_err(|e| e.to_string())?;
    serde_json::from_slice(&reply).map_err(|e| e.to_string())
}

pub fn assistant_state() -> Result<Value, String> {
    send_assistant_cmd(&json!({"command": "get_state"}))
}

/// Current Ollama `/api/ps` model records. Older services fall back to the
/// equivalent `ollama ps` listing.
pub fn assistant_running_models() -> Result<Vec<Value>, String> {
    match send_assistant_cmd(&json!({"command": "list_running_models"})) {
        Ok(reply) => {
            if let Some(models) = reply.get("models").and_then(Value::as_array) {
                return Ok(models.clone());
            }
            // Older installed Assistant binaries do not implement this
            // command. The `ollama ps` fallback below handles that case.
            let unsupported = reply.get("status").and_then(Value::as_str) == Some("error")
                && reply
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|message| message.contains("unknown command list_running_models"));
            if !unsupported {
                eprintln!("[settings] Assistant running-model reply was invalid: {reply}");
            }
        }
        Err(error) => {
            eprintln!(
                "[settings] Assistant running-model query failed; trying `ollama ps`: {error}"
            );
        }
    }
    ollama_cli_running_models()
}

fn ollama_cli_running_models() -> Result<Vec<Value>, String> {
    let output = std::process::Command::new("ollama")
        .arg("ps")
        .output()
        .map_err(|error| format!("run `ollama ps`: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`ollama ps` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let name = line.split_whitespace().next().unwrap_or("unknown");
            json!({"name": name, "display": line.trim()})
        })
        .collect())
}

pub fn assistant_action(command: &str) -> Result<Value, String> {
    if !matches!(command, "mode1" | "mode2" | "interrupt" | "stop") {
        return Err(format!("Unsupported Assistant test action '{command}'"));
    }
    let timeout_ms = if command == "stop" { 30000 } else { 10000 };
    let reply = send_assistant_cmd_with_timeout(&json!({"command": command}), timeout_ms)?;
    if reply.get("status").and_then(Value::as_str) == Some("error") {
        return Err(reply
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Assistant command failed")
            .to_string());
    }
    Ok(reply)
}

pub type AssistantLiveStatus = (Result<Value, String>, Result<Vec<Value>, String>);

pub fn spawn_assistant_status_poller(tx: mpsc::UnboundedSender<AssistantLiveStatus>) {
    thread::spawn(move || loop {
        let status = (assistant_state(), assistant_running_models());
        if tx.unbounded_send(status).is_err() {
            return;
        }
        thread::sleep(Duration::from_secs(2));
    });
}

pub fn spawn_assistant_action(
    command: String,
    tx: mpsc::UnboundedSender<(String, Result<Value, String>)>,
) {
    thread::spawn(move || {
        let result = assistant_action(&command);
        let _ = tx.unbounded_send((command, result));
    });
}

/// Return chat-capable and decision-capable Ollama models, respectively.
/// Capability metadata keeps embeddings and decision models out of the chat
/// selector, and keeps completion models out of the decision-model selector.
pub fn assistant_model_lists() -> Result<(Vec<String>, Vec<String>), String> {
    match send_assistant_cmd(&json!({"command": "list_model_capabilities"})) {
        Ok(reply) => {
            if let Some(models) = reply.get("models").and_then(Value::as_array) {
                let typed: Vec<(String, Vec<String>)> = models
                    .iter()
                    .filter_map(|model| {
                        let name = model.get("name")?.as_str()?.to_string();
                        let capabilities = model
                            .get("capabilities")
                            .and_then(Value::as_array)
                            .map(|items| {
                                items
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_owned)
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        Some((name, capabilities))
                    })
                    .collect();
                if typed
                    .iter()
                    .any(|(_, capabilities)| !capabilities.is_empty())
                {
                    return Ok(partition_model_capabilities(typed));
                }
                eprintln!("[settings] Ollama tag response has no capability metadata; trying `ollama show`");
            }
            eprintln!("[settings] Assistant model-capability reply was invalid: {reply}");
        }
        Err(error) => {
            eprintln!("[settings] Assistant model query failed; using Ollama CLI: {error}");
        }
    }
    Ok(partition_model_capabilities(
        ollama_cli_model_capabilities()?
    ))
}

fn partition_model_capabilities(models: Vec<(String, Vec<String>)>) -> (Vec<String>, Vec<String>) {
    let mut chat = Vec::new();
    let mut decision = Vec::new();
    for (name, capabilities) in models {
        let has = |capability: &str| capabilities.iter().any(|item| item == capability);
        if has("decision") {
            decision.push(name.clone());
        }
        if has("completion") && !has("decision") && !has("embedding") {
            chat.push(name);
        }
    }
    chat.sort();
    chat.dedup();
    decision.sort();
    decision.dedup();
    (chat, decision)
}

fn ollama_cli_model_capabilities() -> Result<Vec<(String, Vec<String>)>, String> {
    let output = std::process::Command::new("ollama")
        .arg("list")
        .output()
        .map_err(|error| format!("run `ollama list`: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`ollama list` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let models = stdout
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut typed = Vec::new();
    for name in models {
        let show = std::process::Command::new("ollama")
            .args(["show", &name])
            .output()
            .map_err(|error| format!("run `ollama show {name}`: {error}"))?;
        if !show.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&show.stdout);
        typed.push((name, parse_show_capabilities(&text)));
    }
    Ok(typed)
}

fn parse_show_capabilities(show: &str) -> Vec<String> {
    let mut in_capabilities = false;
    let mut capabilities = Vec::new();
    for line in show.lines() {
        let line = line.trim();
        if line == "Capabilities" {
            in_capabilities = true;
            continue;
        }
        if !in_capabilities {
            continue;
        }
        if line.is_empty() {
            break;
        }
        if let Some(capability) = line.split_whitespace().next() {
            capabilities.push(capability.to_string());
        }
    }
    capabilities
}

/// List the discovered tool names and current permission levels through the
/// Assistant service. Configured and built-in defaults remain the fallback.
pub fn assistant_tools() -> Result<Vec<(String, String)>, String> {
    let reply = send_assistant_cmd(&json!({"command": "list_tools"}))?;
    let tools = reply
        .get("tools")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("list_tools reply missing tools: {reply}"))?;
    Ok(tools
        .iter()
        .filter_map(|(name, level)| {
            level
                .as_str()
                .map(|level| (name.clone(), level.to_string()))
        })
        .collect())
}

/// Reload model and tool settings in a running Assistant service.
pub fn notify_assistant_reload() -> Result<(), String> {
    let reply = send_assistant_cmd(&json!({"command": "reload_config"}))?;
    if reply.get("status").and_then(Value::as_str) == Some("ok") {
        Ok(())
    } else {
        Err(format!("Assistant reload failed: {reply}"))
    }
}

/// Send a JSON command to the STT service and return its reply.
fn send_stt_cmd(cmd: &Value) -> Result<Value, String> {
    let ctx = zmq::Context::new();
    let socket = ctx.socket(zmq::REQ).map_err(|e| e.to_string())?;
    // ZeroMQ's default infinite linger makes context teardown block forever on
    // an undelivered request, freezing the UI when the service is stopped.
    socket.set_linger(0).map_err(|e| e.to_string())?;
    socket
        .set_sndtimeo(2000)
        .map_err(|e| e.to_string())?;
    socket
        .set_rcvtimeo(2000)
        .map_err(|e| e.to_string())?;
    socket
        .connect(&stt_cmd_addr())
        .map_err(|e| e.to_string())?;
    let payload = serde_json::to_vec(cmd).map_err(|e| e.to_string())?;
    socket.send(&payload, 0).map_err(|e| e.to_string())?;
    let reply = socket.recv_bytes(0).map_err(|e| e.to_string())?;
    serde_json::from_slice(&reply).map_err(|e| e.to_string())
}

/// Ask the running STT service to re-read config.toml so the settings the user
/// just changed apply live. Best-effort: a stopped service is not an error, the
/// change still lands in config.toml and applies on the next start.
pub fn notify_stt_reload() {
    match send_stt_cmd(&json!({"command": "reload_config"})) {
        Ok(reply) => {
            if reply.get("status").and_then(|v| v.as_str()) != Some("ok") {
                eprintln!("[settings] STT reload failed: {reply}");
            }
        }
        Err(error) => {
            eprintln!("[settings] STT reload skipped (service not running?): {error}");
        }
    }
}

/// Send a JSON command to the TTS service and return its reply.
fn send_tts_cmd(cmd: &Value) -> Result<Value, String> {
    let ctx = zmq::Context::new();
    let socket = ctx.socket(zmq::REQ).map_err(|e| e.to_string())?;
    // ZeroMQ's default infinite linger makes context teardown block forever on
    // an undelivered request, freezing the UI when the service is stopped.
    socket.set_linger(0).map_err(|e| e.to_string())?;
    socket
        .set_sndtimeo(2000)
        .map_err(|e| e.to_string())?;
    socket
        .set_rcvtimeo(2000)
        .map_err(|e| e.to_string())?;
    socket
        .connect(&tts_cmd_addr())
        .map_err(|e| e.to_string())?;
    let payload = serde_json::to_vec(cmd).map_err(|e| e.to_string())?;
    socket.send(&payload, 0).map_err(|e| e.to_string())?;
    let reply = socket.recv_bytes(0).map_err(|e| e.to_string())?;
    serde_json::from_slice(&reply).map_err(|e| e.to_string())
}

/// Ask the running TTS service to re-read config.toml so the settings the user
/// just changed apply live. Best-effort: a stopped service is not an error, the
/// change still lands in config.toml and applies on the next start.
pub fn notify_tts_reload() {
    match send_tts_cmd(&json!({"command": "reload_config"})) {
        Ok(reply) => {
            if reply.get("status").and_then(|v| v.as_str()) != Some("ok") {
                eprintln!("[settings] TTS reload failed: {reply}");
            }
        }
        Err(error) => {
            eprintln!("[settings] TTS reload skipped (service not running?): {error}");
        }
    }
}

/// Ask the TTS service to speak `text` with the given settings, returning its
/// reply (normally `{"status": "queued"}`).
pub fn tts_speak(
    text: &str,
    engine: &str,
    voice: &str,
    speed: f64,
    quality: &str,
) -> Result<Value, String> {
    send_tts_cmd(&json!({
        "command": "speak",
        "text": text,
        "engine": engine,
        "voice": voice,
        "speed": speed,
        "quality": quality,
    }))
}

/// Query whether the TTS service is currently playing audio.
pub fn tts_speaking() -> Result<bool, String> {
    let reply = send_tts_cmd(&json!({"command": "get_state"}))?;
    reply
        .get("speaking")
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("get_state reply missing 'speaking': {reply}"))
}

/// Background poller that reports the TTS speaking state. Sends `Some(speaking)`
/// on each successful poll and `None` when the service is unreachable. Keeps
/// running for the app's lifetime; the send side is dropped when the app exits.
pub fn spawn_speaking_poller(tx: mpsc::UnboundedSender<Option<bool>>) {
    thread::spawn(move || loop {
        match tts_speaking() {
            Ok(speaking) => {
                if tx.unbounded_send(Some(speaking)).is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(500));
            }
            Err(_) => {
                if tx.unbounded_send(None).is_err() {
                    return;
                }
                // Service down: the request blocks up to the 2s rcvtimeo before
                // failing, so this throttles itself without spinning.
            }
        }
    });
}

#[cfg(test)]
mod assistant_model_tests {
    use super::{parse_show_capabilities, partition_model_capabilities};

    #[test]
    fn ollama_capabilities_partition_chat_and_decision_models() {
        let (chat, decision) = partition_model_capabilities(vec![
            (
                "chat-model".to_string(),
                vec!["completion".to_string(), "tools".to_string()],
            ),
            ("embedding-model".to_string(), vec!["embedding".to_string()]),
            ("decision-model".to_string(), vec!["decision".to_string()]),
            (
                "multi-capability".to_string(),
                vec!["completion".to_string(), "decision".to_string()],
            ),
        ]);

        assert_eq!(chat, vec!["chat-model".to_string()]);
        assert_eq!(
            decision,
            vec!["decision-model".to_string(), "multi-capability".to_string()]
        );
    }

    #[test]
    fn parses_capabilities_section_from_ollama_show() {
        let output = "  Model\n    architecture qwen\n\n  Capabilities\n    decision\n    tools\n\n  Parameters\n    num_ctx 512\n";
        assert_eq!(
            parse_show_capabilities(output),
            ["decision".to_string(), "tools".to_string()]
        );
    }
}
