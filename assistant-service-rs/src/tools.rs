use serde_json::{json, Value};
use std::{
    collections::{BTreeSet, HashMap},
    fs, path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const EXEC_TIMEOUT_SECS: u64 = 30;

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push_str("…[truncated]");
        out
    }
}

#[derive(Clone, Debug)]
pub struct ToolDef {
    pub name: String,
    pub filepath: String,
    pub description: String,
    pub parameters: Value,
}

impl ToolDef {
    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters,
            }
        })
    }
}

pub struct ToolManager {
    tools: Vec<ToolDef>,
    config: HashMap<String, String>,
    granted: BTreeSet<String>,
    last_pending: Option<String>,
}

impl ToolManager {
    pub fn new(initial: &[(String, String)]) -> Self {
        let mut config = HashMap::new();
        for (name, lvl) in initial {
            config.insert(name.clone(), lvl.clone());
        }
        Self { tools: Vec::new(), config, granted: BTreeSet::new(), last_pending: None }
    }

    pub fn discover(&mut self) {
        let home = std::env::var("HOME").unwrap_or_default();
        let dirs = [
            PathBuf::from(format!("{home}/.local/share/neuropipe/tools")),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tools"),
        ];
        for dir in dirs.iter() {
            if !dir.is_dir() {
                continue;
            }
            let Ok(entries) = fs::read_dir(dir) else { continue };
            for entry in entries.flatten() {
                let tool_dir = entry.path();
                if !tool_dir.is_dir() {
                    continue;
                }
                let name = match tool_dir.file_name() {
                    Some(n) => n.to_string_lossy().into_owned(),
                    None => continue,
                };
                if self.tools.iter().any(|t| t.name == name) {
                    continue;
                }
                let meta = tool_dir.join("tool.json");
                let exec = tool_dir.join("run");
                if !meta.is_file() || !exec.is_file() {
                    continue;
                }
                let Ok(text) = fs::read_to_string(&meta) else { continue };
                let Ok(md) = serde_json::from_str::<Value>(&text) else {
                    eprintln!("[ToolManager] failed to load '{name}'");
                    continue;
                };
                let fn_obj = md.get("function").unwrap_or(&md);
                let definition = ToolDef {
                    name: name.clone(),
                    filepath: exec.display().to_string(),
                    description: fn_obj
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    parameters: fn_obj
                        .get("parameters")
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                };
                if !self.config.contains_key(&name) {
                    let def = md
                        .get("default_permission")
                        .and_then(|v| v.as_str())
                        .unwrap_or("ask");
                    let def = if matches!(def, "allow" | "ask" | "deny") {
                        def.to_string()
                    } else {
                        "ask".to_string()
                    };
                    self.config.insert(name.clone(), def);
                }
                self.tools.push(definition);
            }
        }
    }

    pub fn active_definitions(&self) -> Vec<Value> {
        self.tools
            .iter()
            .filter(|t| self.config.get(&t.name).map(|l| l.as_str()) != Some("deny"))
            .map(|t| t.definition())
            .collect()
    }

    pub fn list_all(&self) -> Value {
        let mut m = serde_json::Map::new();
        for t in &self.tools {
            m.insert(
                t.name.clone(),
                json!(self.config.get(&t.name).map(|s| s.as_str()).unwrap_or("deny")),
            );
        }
        Value::Object(m)
    }

    pub fn set_config(&mut self, cfg: &Value) {
        let Some(obj) = cfg.as_object() else { return };
        for (name, lvl) in obj {
            let Some(l) = lvl.as_str() else { continue };
            if self.tools.iter().any(|t| t.name == *name) && matches!(l, "allow" | "ask" | "deny") {
                self.config.insert(name.clone(), l.to_string());
            }
        }
    }

    pub fn check(&self, name: &str) -> String {
        self.config.get(name).cloned().unwrap_or_else(|| "deny".to_string())
    }

    pub fn is_granted(&self, name: &str) -> bool {
        self.granted.contains(name)
    }

    pub fn grant(&mut self, name: &str) {
        self.granted.insert(name.to_string());
        if self.last_pending.as_deref() == Some(name) {
            self.last_pending = None;
        }
    }

    pub fn mark_pending(&mut self, name: &str) {
        self.last_pending = Some(name.to_string());
    }

    pub fn pending_tool(&self) -> Option<&str> {
        self.last_pending.as_deref()
    }

    pub fn reset_session(&mut self) {
        self.granted.clear();
        self.last_pending = None;
    }

    pub fn execute(&self, name: &str, params: &Value) -> String {
        let tool = match self.tools.iter().find(|t| t.name == name) {
            Some(t) => t,
            None => return format!("Error: unknown tool '{name}'"),
        };
        let params_json = params.to_string();

        let mut child = match Command::new(&tool.filepath)
            .arg(&params_json)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return format!("Error: failed to launch tool '{name}': {e}"),
        };

        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if start.elapsed() >= Duration::from_secs(EXEC_TIMEOUT_SECS) {
                        let _ = child.kill();
                        let _ = child.wait();
                        return format!(
                            "Error: tool '{name}' timed out after {EXEC_TIMEOUT_SECS}s and was killed"
                        );
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return format!("Error: tool '{name}': {e}"),
            }
        };

        let output = match child.wait_with_output() {
            Ok(o) => o,
            Err(e) => return format!("Error: tool '{name}': {e}"),
        };

        let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let detail = if stderr.is_empty() {
                format!("exited with code {:?}", status.code())
            } else {
                format!("exited with code {:?}: {}", status.code(), truncate(&stderr, 1000))
            };
            return format!("Error: tool '{name}' {detail}");
        }
        match serde_json::from_str::<Value>(&out) {
            Ok(data) => {
                if data.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
                    data.get("result")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "(ok)".to_string())
                } else {
                    let msg = data
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    format!("Error: {msg}")
                }
            }
            Err(_) => {
                if out.is_empty() {
                    "(empty output)".to_string()
                } else {
                    out
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_clears_matching_pending() {
        let mut m = ToolManager::new(&[]);
        m.mark_pending("open_url");
        assert_eq!(m.pending_tool(), Some("open_url"));
        m.grant("open_url");
        assert!(m.is_granted("open_url"));
        assert!(m.pending_tool().is_none());
    }

    #[test]
    fn pending_tracks_most_recent() {
        let mut m = ToolManager::new(&[]);
        m.mark_pending("screenshot");
        m.mark_pending("web_search");
        assert_eq!(m.pending_tool(), Some("web_search"));
        m.grant("web_search");
        assert!(m.pending_tool().is_none());
        assert!(!m.is_granted("screenshot"));
    }

    #[test]
    fn reset_session_clears_granted_and_pending() {
        let mut m = ToolManager::new(&[]);
        m.mark_pending("web_search");
        m.grant("open_url");
        m.reset_session();
        assert!(m.pending_tool().is_none());
        assert!(!m.is_granted("open_url"));
    }

    #[test]
    fn discover_validates_default_permission() {
        let mut m = ToolManager::new(&[]);
        m.discover();
        assert_eq!(m.check("open_url"), "deny");
        assert_eq!(m.check("screenshot"), "ask");
    }
}