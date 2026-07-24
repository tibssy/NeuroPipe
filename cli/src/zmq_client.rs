use serde_json::Value;

pub const TTS_CMD: &str = "ipc:///tmp/neuropipe_tts_cmd.sock";
pub const TTS_EVENTS: &str = "ipc:///tmp/neuropipe_tts_events.sock";
pub const STT_CMD: &str = "ipc:///tmp/neuropipe_cmd.sock";
pub const STT_PUB: &str = "ipc:///tmp/neuropipe_pub.sock";
pub const ASSISTANT_CMD: &str = "ipc:///tmp/neuropipe_assistant_cmd.sock";

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
