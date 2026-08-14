use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::{fs, path::PathBuf};

use serde_json::{json, Value};

/// Default TTS command socket, matching the services' fallback config.
const DEFAULT_TTS_CMD: &str = "ipc:///tmp/neuropipe_tts_cmd.sock";

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

/// Send a JSON command to the TTS service and return its reply.
fn send_tts_cmd(cmd: &Value) -> Result<Value, String> {
    let ctx = zmq::Context::new();
    let socket = ctx.socket(zmq::REQ).map_err(|e| e.to_string())?;
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
pub fn spawn_speaking_poller(tx: mpsc::Sender<Option<bool>>) {
    thread::spawn(move || loop {
        match tts_speaking() {
            Ok(speaking) => {
                if tx.send(Some(speaking)).is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(500));
            }
            Err(_) => {
                if tx.send(None).is_err() {
                    return;
                }
                // Service down: the request blocks up to the 2s rcvtimeo before
                // failing, so this throttles itself without spinning.
            }
        }
    });
}
