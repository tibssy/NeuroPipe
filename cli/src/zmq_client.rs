use std::{env, fs, path::PathBuf, sync::OnceLock};

use serde_json::Value;
use toml::Value as TomlValue;

struct IpcAddrs {
    tts_cmd: String,
    tts_events: String,
    stt_cmd: String,
    stt_pub: String,
    assistant_cmd: String,
}

static IPC_ADDRS: OnceLock<IpcAddrs> = OnceLock::new();

fn defaults() -> IpcAddrs {
    IpcAddrs {
        tts_cmd: "ipc:///tmp/neuropipe_tts_cmd.sock".to_string(),
        tts_events: "ipc:///tmp/neuropipe_tts_events.sock".to_string(),
        stt_cmd: "ipc:///tmp/neuropipe_cmd.sock".to_string(),
        stt_pub: "ipc:///tmp/neuropipe_pub.sock".to_string(),
        assistant_cmd: "ipc:///tmp/neuropipe_assistant_cmd.sock".to_string(),
    }
}

fn config_path() -> Option<PathBuf> {
    env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".config/neuropipe/config.toml"))
}

fn load_from_config() -> IpcAddrs {
    let mut addrs = defaults();
    let Some(path) = config_path() else {
        return addrs;
    };

    let Ok(raw) = fs::read_to_string(path) else {
        return addrs;
    };

    let Ok(parsed) = raw.parse::<TomlValue>() else {
        return addrs;
    };

    let Some(ipc) = parsed.get("ipc") else {
        return addrs;
    };

    if let Some(v) = ipc.get("tts_cmd").and_then(|v| v.as_str()) {
        addrs.tts_cmd = v.to_string();
    }
    if let Some(v) = ipc.get("tts_events").and_then(|v| v.as_str()) {
        addrs.tts_events = v.to_string();
    }
    if let Some(v) = ipc.get("stt_cmd").and_then(|v| v.as_str()) {
        addrs.stt_cmd = v.to_string();
    }
    if let Some(v) = ipc.get("stt_pub").and_then(|v| v.as_str()) {
        addrs.stt_pub = v.to_string();
    }
    if let Some(v) = ipc.get("assistant_cmd").and_then(|v| v.as_str()) {
        addrs.assistant_cmd = v.to_string();
    }

    addrs
}

fn get_addrs() -> &'static IpcAddrs {
    IPC_ADDRS.get_or_init(load_from_config)
}

pub fn tts_cmd() -> &'static str {
    &get_addrs().tts_cmd
}

pub fn tts_events() -> &'static str {
    &get_addrs().tts_events
}

pub fn stt_cmd() -> &'static str {
    &get_addrs().stt_cmd
}

pub fn stt_pub() -> &'static str {
    &get_addrs().stt_pub
}

pub fn assistant_cmd() -> &'static str {
    &get_addrs().assistant_cmd
}

pub fn send_cmd(addr: &str, cmd: &Value) -> Result<Value, String> {
    let ctx = zmq::Context::new();
    let socket = ctx.socket(zmq::REQ).map_err(|e| e.to_string())?;
    socket.set_rcvtimeo(5000).map_err(|e| e.to_string())?;
    socket.connect(addr).map_err(|e| e.to_string())?;
    let payload = serde_json::to_vec(cmd).map_err(|e| e.to_string())?;
    socket.send(&payload, 0).map_err(|e| e.to_string())?;
    let reply = socket.recv_bytes(0).map_err(|e| format!("{} (is the service running?)", e))?;
    serde_json::from_slice(&reply).map_err(|e| e.to_string())
}

pub fn create_sub(ctx: &zmq::Context, addr: &str) -> Result<zmq::Socket, String> {
    let sub = ctx.socket(zmq::SUB).map_err(|e| e.to_string())?;
    sub.connect(addr).map_err(|e| e.to_string())?;
    sub.set_subscribe(b"").map_err(|e| e.to_string())?;
    Ok(sub)
}
