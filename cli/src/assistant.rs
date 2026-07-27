use serde_json::{json, Value};
use crate::zmq_client;
use crate::config;

pub fn start(mode: &str, model: Option<&str>, engine: Option<&str>, voice: Option<&str>) {
    // Check if busy and interrupt if needed
    match zmq_client::send_cmd(zmq_client::assistant_cmd(), &json!({"command": "get_state"})) {
        Ok(state) => {
            if state.get("busy").and_then(|v| v.as_bool()).unwrap_or(false) {
                let _ = zmq_client::send_cmd(zmq_client::assistant_cmd(), &json!({"command": "interrupt"}));
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

    match zmq_client::send_cmd(zmq_client::assistant_cmd(), &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn interrupt() {
    let cmd = json!({"command": "interrupt"});
    match zmq_client::send_cmd(zmq_client::assistant_cmd(), &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn stop() {
    let cmd = json!({"command": "stop"});
    match zmq_client::send_cmd(zmq_client::assistant_cmd(), &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn get_state() {
    let cmd = json!({"command": "get_state"});
    match zmq_client::send_cmd(zmq_client::assistant_cmd(), &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }
}

fn cycle_model(direction: &str) -> Option<String> {
    let reply = zmq_client::send_cmd(zmq_client::assistant_cmd(), &json!({"command": "list_models"})).ok()?;
    let models = reply.get("models")?.as_array()?;
    if models.is_empty() {
        eprintln!("No models available.");
        return None;
    }

    let mut model_names: Vec<String> = models
        .iter()
        .filter_map(|v| v.as_str())
        .map(|s| s.to_string())
        .collect();

    match config::favorite_models() {
        Ok(favorites) if !favorites.is_empty() => {
            let available: std::collections::HashSet<String> = model_names.iter().cloned().collect();
            let filtered: Vec<String> = favorites
                .into_iter()
                .filter(|m| available.contains(m))
                .collect();
            if filtered.is_empty() {
                eprintln!("No available favorite models found.");
                return None;
            }
            model_names = filtered;
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("Warning: failed to read favorite models from config: {}", e);
        }
    }

    let current = zmq_client::send_cmd(zmq_client::assistant_cmd(), &json!({"command": "get_state"})).ok()?;
    let current_model = current.get("model").and_then(|v| v.as_str()).unwrap_or("");

    let idx = model_names.iter().position(|v| v == current_model);
    let new_idx = match (idx, direction) {
        (Some(i), "next") => (i + 1) % model_names.len(),
        (Some(i), "prev") => (i + model_names.len() - 1) % model_names.len(),
        (None, "next") | (None, "prev") => 0,
        _ => return None,
    };

    Some(model_names[new_idx].clone())
}

pub fn list_models() {
    let cmd = json!({"command": "list_models"});
    match zmq_client::send_cmd(zmq_client::assistant_cmd(), &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn list_tools() {
    let cmd = json!({"command": "list_tools"});
    match zmq_client::send_cmd(zmq_client::assistant_cmd(), &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn set_tools(config_json: &str) {
    let tools: Value = match serde_json::from_str(config_json) {
        Ok(v) => v,
        Err(e) => { eprintln!("Invalid JSON: {}", e); return; }
    };
    let cmd = json!({"command": "set_tools", "tools": tools});
    match zmq_client::send_cmd(zmq_client::assistant_cmd(), &cmd) {
        Ok(reply) => {
            let is_error = reply.get("status").and_then(|v| v.as_str()) == Some("error");
            if !is_error {
                if let Some(tools_reply) = reply.get("tools") {
                    if let Err(e) = config::persist_assistant_tools(tools_reply) {
                        eprintln!("Warning: tools updated in service, but failed to persist config: {}", e);
                    }
                } else {
                    eprintln!("Warning: tools updated in service, but reply missing tools for config persistence.");
                }
            }
            println!("{}", serde_json::to_string_pretty(&reply).unwrap())
        }
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn grant_tool(tool: &str) {
    let cmd = json!({"command": "grant_tool", "tool": tool});
    match zmq_client::send_cmd(zmq_client::assistant_cmd(), &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn deny_tool(tool: &str) {
    let cmd = json!({"command": "deny_tool", "tool": tool});
    match zmq_client::send_cmd(zmq_client::assistant_cmd(), &cmd) {
        Ok(reply) => println!("{}", serde_json::to_string_pretty(&reply).unwrap()),
        Err(e) => eprintln!("Error: {}", e),
    }
}

pub fn set_model(model: &str) {
    let resolved = match model {
        "next" | "prev" => match cycle_model(model) {
            Some(m) => {
                eprintln!("Model: {}", m);
                m
            }
            None => return,
        },
        _ => model.to_string(),
    };

    let cmd = json!({"command": "set_model", "model": resolved});
    match zmq_client::send_cmd(zmq_client::assistant_cmd(), &cmd) {
        Ok(reply) => {
            let is_error = reply.get("status").and_then(|v| v.as_str()) == Some("error");
            if !is_error {
                if let Some(model) = reply.get("model").and_then(|v| v.as_str()) {
                    if let Err(e) = config::persist_assistant_model(model) {
                        eprintln!("Warning: model updated in service, but failed to persist config: {}", e);
                    }
                } else {
                    eprintln!("Warning: model updated in service, but reply missing model for config persistence.");
                }
            }
            println!("{}", serde_json::to_string_pretty(&reply).unwrap())
        }
        Err(e) => eprintln!("Error: {}", e),
    }
}
