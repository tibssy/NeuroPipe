use serde_json::json;
use crate::zmq_client;

pub fn start(mode: &str, model: Option<&str>, engine: Option<&str>, voice: Option<&str>) {
    // Check if busy and interrupt if needed
    match zmq_client::send_cmd(zmq_client::ASSISTANT_CMD, &json!({"command": "get_state"})) {
        Ok(state) => {
            if state.get("busy").and_then(|v| v.as_bool()).unwrap_or(false) {
                let _ = zmq_client::send_cmd(zmq_client::ASSISTANT_CMD, &json!({"command": "interrupt"}));
            }
        }
        Err(e) => {
            eprintln!("Warning: {}", e);
        }
    }

    let mut cmd = json!({"command": mode});
    if let Some(v) = model { cmd["model"] = json!(v); }
    if let Some(v) = engine { cmd["engine"] = json!(v); }
    if let Some(v) = voice { cmd["voice"] = json!(v); }

    match zmq_client::send_cmd(zmq_client::ASSISTANT_CMD, &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn interrupt() {
    let cmd = json!({"command": "interrupt"});
    match zmq_client::send_cmd(zmq_client::ASSISTANT_CMD, &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn stop() {
    let cmd = json!({"command": "stop"});
    match zmq_client::send_cmd(zmq_client::ASSISTANT_CMD, &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn get_state() {
    let cmd = json!({"command": "get_state"});
    match zmq_client::send_cmd(zmq_client::ASSISTANT_CMD, &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }
}
