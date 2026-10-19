use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::config;
use crate::zmq_client;

/// Friendly STT model names: (alias, `[stt].model`, `[stt].model_dir`).
const STT_MODELS: &[(&str, &str, &str)] = &[
    (
        "parakeet-v3",
        "nemo-parakeet-tdt-0.6b-v3",
        "~/.local/share/neuropipe/stt/parakeet-v3",
    ),
    (
        "parakeet-redux",
        "nemo-parakeet-tdt-0.6b-v3-redux",
        "~/.local/share/neuropipe/stt/parakeet-redux",
    ),
];

fn resolve_model(name: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match name.trim().to_ascii_lowercase().as_str() {
        "parakeet-v3" | "v3" | "nemo-parakeet-tdt-0.6b-v3" => Some(STT_MODELS[0]),
        "parakeet-redux" | "redux" | "nemo-parakeet-tdt-0.6b-v3-redux"
        | "parakeet-tdt-0.6b-redux-onnx" => Some(STT_MODELS[1]),
        _ => None,
    }
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(path),
    }
}

/// Whether the directory holds a loadable set of graphs: the engine accepts
/// `encoder-model[.int8|.int4].onnx` and `decoder_joint-model[.int8|.int4].onnx`.
fn model_files_present(dir: &Path) -> bool {
    let has_graph = |stem: &str| {
        ["int8", "int4", ""].iter().any(|suffix| {
            let name = if suffix.is_empty() {
                format!("{stem}.onnx")
            } else {
                format!("{stem}.{suffix}.onnx")
            };
            dir.join(name).is_file()
        })
    };
    dir.join("vocab.txt").is_file() && has_graph("encoder-model") && has_graph("decoder_joint-model")
}

/// Switch the ASR model in config.toml and ask a running service to reload.
/// The service retargets its lazily loaded engine, so the next transcription
/// uses the new model without a restart.
pub fn set_model(name: &str) {
    let Some((alias, model, model_dir)) = resolve_model(name) else {
        eprintln!("Unknown STT model '{name}'. Available models:");
        for (alias, model, _) in STT_MODELS {
            eprintln!("  {alias:<16} {model}");
        }
        std::process::exit(1);
    };

    if let Err(error) = config::persist_stt_model(model, model_dir) {
        eprintln!("Error: failed to update config: {error}");
        std::process::exit(1);
    }

    let dir = expand_home(model_dir);
    println!("STT model set to {alias} ({model})");
    println!("Model directory: {}", dir.display());

    if !model_files_present(&dir) {
        eprintln!(
            "Warning: no model files found in {}; run install.sh or stt-service-rs/fetch-model.sh.",
            dir.display()
        );
    }

    // Tell a running service to re-read config.toml; it retargets the engine so
    // the next transcription loads this model (no restart needed).
    let running = match zmq_client::send_cmd_with_timeout(
        zmq_client::stt_cmd(),
        &json!({"command": "reload_config"}),
        2000,
    ) {
        Ok(reply) => {
            if reply.get("status").and_then(Value::as_str) != Some("ok") {
                eprintln!("Warning: service rejected reload_config: {reply}");
            }
            true
        }
        Err(_) => {
            println!("STT service is not running; the change applies on next start.");
            false
        }
    };

    if running {
        println!("Reloaded; the next transcription will use {alias}.");
    }
}

fn send_set_mode(mode: &str) {
    let cmd = json!({"command": "set_mode", "mode": mode});
    match zmq_client::send_cmd(zmq_client::stt_cmd(), &cmd) {
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

    let ctx = zmq::Context::new();
    let sub = match zmq_client::create_sub(&ctx, zmq_client::stt_pub()) {
        Ok(s) => s,
        Err(e) => { eprintln!("Error: {}", e); send_set_mode("IDLE"); return; }
    };
    // Subscribe before enabling VAD so the listening_start event is not lost.
    send_set_mode("VAD");

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
    match zmq_client::send_cmd(zmq_client::stt_cmd(), &cmd) {
        Ok(_) => {}
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn listen() {
    let ctx = zmq::Context::new();
    let sub = match zmq_client::create_sub(&ctx, zmq_client::stt_pub()) {
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

#[cfg(test)]
mod tests {
    use super::resolve_model;

    #[test]
    fn model_aliases_resolve_to_expected_directories() {
        let (_, model, dir) = resolve_model("parakeet-v3").unwrap();
        assert_eq!(model, "nemo-parakeet-tdt-0.6b-v3");
        assert_eq!(dir, "~/.local/share/neuropipe/stt/parakeet-v3");

        let (_, model, dir) = resolve_model("redux").unwrap();
        assert_eq!(model, "nemo-parakeet-tdt-0.6b-v3-redux");
        assert_eq!(dir, "~/.local/share/neuropipe/stt/parakeet-redux");

        assert!(resolve_model("not-a-model").is_none());
    }
}
