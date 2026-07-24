use serde_json::{json, Value};
use crate::zmq_client;

fn send_set_mode(mode: &str) {
    let cmd = json!({"command": "set_mode", "mode": mode});
    match zmq_client::send_cmd(zmq_client::STT_CMD, &cmd) {
        Ok(reply) => {
            if reply.get("status").and_then(|v| v.as_str()) != Some("ok") {
                eprintln!("Unexpected reply: {}", serde_json::to_string(&reply).unwrap());
            }
        }
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn trigger() {
    eprintln!("Listening...");
    send_set_mode("VAD");

    let ctx = zmq::Context::new();
    let sub = match zmq_client::create_sub(&ctx, zmq_client::STT_PUB) {
        Ok(s) => s,
        Err(e) => { eprintln!("Error: {}", e); send_set_mode("IDLE"); return; }
    };

    loop {
        match sub.recv_bytes(0) {
            Ok(bytes) => {
                if let Ok(event) = serde_json::from_slice::<Value>(&bytes) {
                    let event_type = event.get("event").and_then(|v| v.as_str()).unwrap_or("");
                    match event_type {
                        "listening_start" => eprintln!("Voice Detected..."),
                        "transcription" => {
                            if let Some(text) = event.get("text").and_then(|v| v.as_str()) {
                                println!("{}", text);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            Err(_) => break,
        }
    }
    send_set_mode("IDLE");
}

pub fn vad() {
    send_set_mode("VAD");
}

pub fn idle() {
    send_set_mode("IDLE");
}

pub fn record_start() {
    send_set_mode("MANUAL");
}

pub fn record_stop() {
    let cmd = json!({"command": "manual_stop"});
    match zmq_client::send_cmd(zmq_client::STT_CMD, &cmd) {
        Ok(_) => {}
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn listen() {
    let ctx = zmq::Context::new();
    let sub = match zmq_client::create_sub(&ctx, zmq_client::STT_PUB) {
        Ok(s) => s,
        Err(e) => { eprintln!("Error: {}", e); return; }
    };
    eprintln!("NeuroPipe Listener Connected...");
    loop {
        match sub.recv_bytes(0) {
            Ok(bytes) => {
                if let Ok(event) = serde_json::from_slice::<Value>(&bytes) {
                    let event_type = event.get("event").and_then(|v| v.as_str()).unwrap_or("");
                    match event_type {
                        "transcription" => {
                            if let Some(text) = event.get("text").and_then(|v| v.as_str()) {
                                println!("User: {}", text);
                            }
                        }
                        "listening_start" => {
                            eprint!("\r...");
                            let _ = std::io::Write::flush(&mut std::io::stderr());
                        }
                        _ => {}
                    }
                }
            }
            Err(_) => break,
        }
    }
}
