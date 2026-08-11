mod config;
mod markdown;
mod memory;
mod ollama;
mod service;
mod tools;

use std::{sync::Arc, thread};

use serde_json::{json, Value};

use config::Config;
use service::Shared;

fn main() {
    let cfg = Config::load();
    let shared = Arc::new(Shared::new(cfg.clone()));

    // seed system message (no tools at boot)
    let initial = shared.build_system_message(false);
    shared.push_history(initial);

    println!("NeuroPipe is ready.");
    println!("Model: {}", shared.model.lock().unwrap().clone());
    println!("Socket: {}", cfg.ipc.assistant_cmd);

    service::start_tts_event_listener(&shared);
    let c = Arc::clone(&shared);
    thread::spawn(move || cmd_loop(&c));
    let s = Arc::clone(&shared);
    thread::spawn(move || stt_loop(&s));

    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

fn cmd_loop(shared: &Arc<Shared>) {
    let addr = shared.cfg.ipc.assistant_cmd.clone();
    let ctx = zmq::Context::new();
    let sock = match ctx.socket(zmq::REP) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[cmd] failed to create REP socket: {e}");
            return;
        }
    };
    if let Err(e) = sock.bind(&addr) {
        eprintln!("[cmd] failed to bind {addr}: {e}");
        return;
    }
    loop {
        match sock.recv_bytes(0) {
            Ok(payload) => {
                let reply = match serde_json::from_slice::<Value>(&payload) {
                    Ok(cmd) => handle_cmd(shared, &cmd),
                    Err(_) => json!({"status": "error", "message": "invalid json"}),
                };
                match serde_json::to_vec(&reply) {
                    Ok(bytes) => {
                        let _ = sock.send(&bytes, 0);
                    }
                    Err(_) => {
                        let _ = sock.send(&b"{}"[..], 0);
                    }
                }
            }
            Err(_) => {}
        }
    }
}

fn handle_cmd(shared: &Arc<Shared>, cmd: &Value) -> Value {
    let command = cmd.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string();
    match command.as_str() {
        "mode1" | "mode2" => {
            if shared.is_busy() {
                shared.interrupt();
            }
            let mode = if command == "mode1" { "MODE1" } else { "MODE2" };
            shared.start_session(
                mode,
                cmd.get("model").and_then(|v| v.as_str()).map(|s| s.to_string()),
                cmd.get("engine").and_then(|v| v.as_str()).map(|s| s.to_string()),
                cmd.get("voice").and_then(|v| v.as_str()).map(|s| s.to_string()),
            );
            json!({"status": "ok", "mode": mode})
        }
        "interrupt" => {
            let last = shared.interrupt();
            json!({"status": "interrupted", "last_sentence": last})
        }
        "stop" => {
            shared.stop();
            json!({"status": "stopped"})
        }
        "list_models" => {
            let models = shared.list_models();
            json!({"models": models})
        }
        "set_model" => {
            if let Some(m) = cmd.get("model").and_then(|v| v.as_str()) {
                if !m.trim().is_empty() {
                    *shared.model.lock().unwrap() = m.trim().to_string();
                }
            }
            json!({"model": shared.model.lock().unwrap().clone()})
        }
        "list_tools" => {
            json!({"tools": shared.tools.lock().unwrap().list_all()})
        }
        "set_tools" => {
            let cfg = match cmd.get("tools") {
                Some(c) if c.is_object() => c,
                _ => return json!({"status": "error", "message": "tools must be an object"}),
            };
            let known: Vec<String> = shared
                .tools
                .lock()
                .unwrap()
                .list_all()
                .as_object()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            if let Some(obj) = cfg.as_object() {
                for (name, level) in obj {
                    if !known.contains(&name.clone()) {
                        return json!({
                            "status": "error",
                            "message": format!("Unknown tool '{name}'")
                        });
                    }
                    if !matches!(level.as_str().unwrap_or(""), "allow" | "ask" | "deny") {
                        return json!({
                            "status": "error",
                            "message": format!("Invalid permission '{level}' for tool '{name}'")
                        });
                    }
                }
            }
            shared.tools.lock().unwrap().set_config(cfg);
            json!({"tools": shared.tools.lock().unwrap().list_all()})
        }
        "grant_tool" => {
            let name = cmd.get("tool").and_then(|v| v.as_str()).unwrap_or("");
            shared.tools.lock().unwrap().grant(name);
            json!({"status": "granted", "tool": name})
        }
        "deny_tool" => {
            json!({
                "status": "denied",
                "tool": cmd.get("tool").and_then(|v| v.as_str()).unwrap_or("")
            })
        }
        "get_state" => {
            let tts_state = shared.get_tts_state();
            json!({
                "mode": shared.mode.lock().unwrap().clone(),
                "busy": shared.is_busy(),
                "model": shared.model.lock().unwrap().clone(),
                "engine": tts_state.get("engine"),
                "voice": tts_state.get("voice"),
                "speed": tts_state.get("speed"),
                "quality": tts_state.get("quality"),
            })
        }
        "get_history" => {
            let history = shared.history_json();
            let count = history.len();
            json!({"count": count, "history": history})
        }
        "reset_memory" => {
            shared.reset_memory()
        }
        _ => json!({"status": "error", "message": format!("unknown command {command}")}),
    }
}

fn stt_loop(shared: &Arc<Shared>) {
    let addr = shared.cfg.ipc.stt_pub.clone();
    let ctx = zmq::Context::new();
    let sub = match ctx.socket(zmq::SUB) {
        Ok(s) => s,
        Err(_) => return,
    };
    if sub.connect(&addr).is_err() {
        return;
    }
    if sub.set_subscribe(b"").is_err() {
        return;
    }
    loop {
        match sub.recv_string(0) {
            Ok(Ok(frame)) => {
                let Ok(msg) = serde_json::from_str::<Value>(&frame) else { continue };
                let event = msg.get("event").and_then(|v| v.as_str()).unwrap_or("");
                match event {
                    "transcription" => {
                        let text = msg.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let mode = shared.mode.lock().unwrap().clone();
                        if (mode == "MODE1" || mode == "MODE2") && !text.is_empty() {
                            run_transcription(shared, text);
                        }
                    }
                    "listening_start" => {
                        print!(".");
                        use std::io::Write;
                        let _ = std::io::stdout().flush();
                        // Instant barge-in: in MODE2, stop TTS the moment speech
                        // onset is detected, rather than after the full
                        // transcription arrives. MODE1 stays half-duplex.
                        {
                            let mode = shared.mode.lock().unwrap().clone();
                            if mode == "MODE2"
                                && (shared.is_busy() || shared.tts_events.is_speaking())
                            {
                                println!("\n[Interrupting — speech detected]");
                                shared.interrupt();
                            }
                        }
                        let sh = Arc::clone(shared);
                        std::thread::spawn(move || sh.warm_tts());
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn run_transcription(shared: &Arc<Shared>, text: String) {
    if shared.is_busy() {
        if shared.mode.lock().unwrap().clone() == "MODE1" {
            println!("\n[Busy — ignoring new input]");
            return;
        }
        shared.interrupt();
    }
    shared.cancel.store(false, std::sync::atomic::Ordering::SeqCst);
    *shared.busy.lock().unwrap() = true;
    let sh = Arc::clone(shared);
    std::thread::spawn(move || {
        sh.process_and_respond(text);
        *sh.busy.lock().unwrap() = false;
    });
}
