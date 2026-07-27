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
        let sub = match zmq_client::create_sub(&ctx, zmq_client::tts_events()) {
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

    match zmq_client::send_cmd(zmq_client::tts_cmd(), &cmd) {
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
    match zmq_client::send_cmd(zmq_client::tts_cmd(), &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn get_state() {
    let cmd = json!({"command": "get_state"});
    match zmq_client::send_cmd(zmq_client::tts_cmd(), &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }
}

fn cycle_voice(direction: &str, engine: Option<&str>) -> Option<String> {
    let state = zmq_client::send_cmd(zmq_client::tts_cmd(), &json!({"command": "get_state"})).ok()?;
    let current_voice = state.get("voice").and_then(|v| v.as_str()).unwrap_or("");
    let eng = engine.or_else(|| state.get("engine").and_then(|v| v.as_str())).unwrap_or("pocket-tts");

    let mut list_cmd = json!({"command": "list_voices"});
    list_cmd["engine"] = json!(eng);
    let reply = zmq_client::send_cmd(zmq_client::tts_cmd(), &list_cmd).ok()?;
    let voices = reply.get("voices")?.as_array()?;
    if voices.is_empty() {
        eprintln!("No voices available.");
        return None;
    }

    let voice_names: Vec<&str> = voices.iter().filter_map(|v| v.as_str()).collect();

    let current_base = std::path::Path::new(current_voice)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(current_voice);

    let idx = voice_names.iter().position(|v| *v == current_base);
    let new_idx = match (idx, direction) {
        (Some(i), "next") => (i + 1) % voice_names.len(),
        (Some(i), "prev") => (i + voice_names.len() - 1) % voice_names.len(),
        (None, "next") | (None, "prev") => 0,
        _ => return None,
    };

    Some(voice_names[new_idx].to_string())
}

pub fn set_state(engine: Option<&str>, voice: Option<&str>, speed: Option<f64>, quality: Option<&str>) {
    let resolved_voice = match voice {
        Some("next") | Some("prev") => {
            match cycle_voice(voice.unwrap(), engine) {
                Some(v) => {
                    eprintln!("Voice: {}", v);
                    Some(v)
                }
                None => {
                    eprintln!("No voices available.");
                    return;
                }
            }
        }
        v => v.map(|s| s.to_string()),
    };

    let mut cmd = json!({"command": "set_state"});
    if let Some(v) = engine { cmd["engine"] = json!(v); }
    if let Some(v) = &resolved_voice { cmd["voice"] = json!(v); }
    if let Some(s) = speed { cmd["speed"] = json!(s); }
    if let Some(q) = quality { cmd["quality"] = json!(q); }
    match zmq_client::send_cmd(zmq_client::tts_cmd(), &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn monitor() {
    let ctx = zmq::Context::new();
    let sub = match zmq_client::create_sub(&ctx, zmq_client::tts_events()) {
        Ok(s) => s,
        Err(e) => { eprintln!("Error: {}", e); return; }
    };
    eprintln!("Listening for events on {}...", zmq_client::tts_events());
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
