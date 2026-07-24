use std::io::{self, Write};
use serde_json::{json, Value};
use crate::zmq_client;

pub fn speak(text: &str, voice: Option<&str>, speed: Option<f64>, quality: Option<&str>, engine: Option<&str>, monitor: bool) {
    let mut cmd = json!({"command": "speak", "text": text});
    if let Some(v) = voice { cmd["voice"] = json!(v); }
    if let Some(s) = speed { cmd["speed"] = json!(s); }
    if let Some(q) = quality { cmd["quality"] = json!(q); }
    if let Some(e) = engine { cmd["engine"] = json!(e); }

    if monitor {
        let ctx = zmq::Context::new();
        let sub = match zmq_client::create_sub(&ctx, zmq_client::TTS_EVENTS) {
            Ok(s) => s,
            Err(e) => { eprintln!("Event listener: {}", e); return; }
        };
        std::thread::spawn(move || {
            loop {
                match sub.recv_bytes(0) {
                    Ok(bytes) => {
                        if let Ok(event) = serde_json::from_slice::<Value>(&bytes) {
                            let event_type = event.get("event").and_then(|v| v.as_str()).unwrap_or("");
                            match event_type {
                                "speaking" => {
                                    if let Some(s) = event.get("sentence").and_then(|v| v.as_str()) {
                                        eprintln!("Speaking: '{}'", s);
                                    }
                                }
                                "interrupted" => {
                                    eprintln!("INTERRUPTED");
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }

    match zmq_client::send_cmd(zmq_client::TTS_CMD, &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }

    if monitor {
        print!("\nPress Enter to exit...");
        io::stdout().flush().ok();
        let mut input = String::new();
        io::stdin().read_line(&mut input).ok();
    }
}

pub fn stop() {
    let cmd = json!({"command": "stop"});
    match zmq_client::send_cmd(zmq_client::TTS_CMD, &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn get_state() {
    let cmd = json!({"command": "get_state"});
    match zmq_client::send_cmd(zmq_client::TTS_CMD, &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn set_state(engine: Option<&str>, voice: Option<&str>, speed: Option<f64>, quality: Option<&str>) {
    let mut cmd = json!({"command": "set_state"});
    if let Some(v) = engine { cmd["engine"] = json!(v); }
    if let Some(v) = voice { cmd["voice"] = json!(v); }
    if let Some(s) = speed { cmd["speed"] = json!(s); }
    if let Some(q) = quality { cmd["quality"] = json!(q); }
    match zmq_client::send_cmd(zmq_client::TTS_CMD, &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn monitor() {
    let ctx = zmq::Context::new();
    let sub = match zmq_client::create_sub(&ctx, zmq_client::TTS_EVENTS) {
        Ok(s) => s,
        Err(e) => { eprintln!("Error: {}", e); return; }
    };
    eprintln!("Listening for events on {}...", zmq_client::TTS_EVENTS);
    loop {
        match sub.recv_bytes(0) {
            Ok(bytes) => {
                if let Ok(event) = serde_json::from_slice::<Value>(&bytes) {
                    let event_type = event.get("event").and_then(|v| v.as_str()).unwrap_or("unknown");
                    match event_type {
                        "speaking" => {
                            let sentence = event.get("sentence").and_then(|v| v.as_str()).unwrap_or("");
                            eprintln!("Speaking: '{}'", sentence);
                        }
                        "sentence_done" => {}
                        "interrupted" => {
                            let last = event.get("last_sentence").and_then(|v| v.as_str()).unwrap_or("");
                            eprintln!("INTERRUPTED at: '{}'", last);
                        }
                        _ => eprintln!("Event: {}", serde_json::to_string(&event).unwrap()),
                    }
                }
            }
            Err(_) => break,
        }
    }
}
