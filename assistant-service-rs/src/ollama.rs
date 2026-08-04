use serde_json::{json, Value};
use std::io::{BufRead, BufReader};

pub struct OllamaClient {
    client: reqwest::blocking::Client,
    base: String,
}

#[derive(Clone, Debug)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug)]
pub enum OllamaError {
    Http(String),
    Parse(String),
}

impl std::fmt::Display for OllamaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OllamaError::Http(s) => write!(f, "{s}"),
            OllamaError::Parse(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for OllamaError {}

/// A lazy stream of NDJSON lines from /api/chat.
pub struct ChatStream {
    reader: BufReader<reqwest::blocking::Response>,
}

impl ChatStream {
    /// Returns the next parsed chunk, or Ok(None) at end of stream.
    pub fn next_chunk(&mut self) -> Result<Option<ChatChunk>, OllamaError> {
        let mut line = String::new();
        loop {
            line.clear();
            let n = self
                .reader
                .read_line(&mut line)
                .map_err(|e| OllamaError::Parse(e.to_string()))?;
            if n == 0 {
                return Ok(None);
            }
            if line.trim().is_empty() {
                continue;
            }
            let v: Value = serde_json::from_str(&line).map_err(|e| OllamaError::Parse(e.to_string()))?;
            if v.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
                return Ok(None);
            }
            return Ok(Some(parse_chunk(&v)));
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChatChunk {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

fn parse_chunk(v: &Value) -> ChatChunk {
    let content = v
        .pointer("/message/content")
        .and_then(|c| c.as_str())
        .map(|s| s.to_string());
    let mut tool_calls = Vec::new();
    if let Some(tcs) = v.pointer("/message/tool_calls").and_then(|t| t.as_array()) {
        for tc in tcs {
            let name = tc
                .pointer("/function/name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            let arguments = tc
                .pointer("/function/arguments")
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default()));
            tool_calls.push(ToolCall { name, arguments });
        }
    }
    ChatChunk { content, tool_calls }
}

impl OllamaClient {
    pub fn new() -> Self {
        // Ollama exposes a TCP endpoint by default. (The Python service preferred
        // the unix socket when present, but reqwest has no native UDS transport;
        // localhost:11434 is equivalent and always available.)
        let base = "http://localhost:11434".to_string();
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .expect("build ollama http client");
        Self { client, base }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    pub fn chat_stream(
        &self,
        model: &str,
        messages: &[Value],
        tools: Option<&[Value]>,
    ) -> Result<ChatStream, OllamaError> {
        let mut body = json!({
            "model": model,
            "messages": messages,
            "stream": true,
        });
        if let Some(t) = tools {
            body["tools"] = Value::Array(t.to_vec());
        }
        let resp = self
            .client
            .post(self.url("/api/chat"))
            .json(&body)
            .send()
            .map_err(|e| OllamaError::Http(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(OllamaError::Http(format!("/api/chat returned {status}")));
        }
        Ok(ChatStream { reader: BufReader::new(resp) })
    }

    /// Non-streaming chat; returns the concatenated message content.
    pub fn chat_non_stream(&self, model: &str, messages: &[Value]) -> Result<String, OllamaError> {
        let body = json!({
            "model": model,
            "messages": messages,
            "stream": false,
        });
        let resp = self
            .client
            .post(self.url("/api/chat"))
            .json(&body)
            .send()
            .map_err(|e| OllamaError::Http(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(OllamaError::Http(format!("/api/chat returned {status}")));
        }
        let v: Value = resp
            .json()
            .map_err(|e| OllamaError::Parse(e.to_string()))?;
        let content = v
            .pointer("/message/content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        Ok(content)
    }

    pub fn embed(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let body = json!({ "model": model, "input": texts });
        let resp = self
            .client
            .post(self.url("/api/embed"))
            .json(&body)
            .send()
            .map_err(|e| e.to_string())?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("ollama /api/embed returned {status}"));
        }
        let v: Value = resp.json().map_err(|e| e.to_string())?;
        let emb = v
            .get("embeddings")
            .and_then(|x| x.as_array())
            .ok_or_else(|| "ollama embed response missing embeddings".to_string())?;
        let mut out = Vec::new();
        for e in emb {
            let mut vec = Vec::new();
            for n in e.as_array().ok_or("embedding not array")? {
                vec.push(n.as_f64().unwrap_or(0.0) as f32);
            }
            out.push(vec);
        }
        Ok(out)
    }

    pub fn list_models(&self) -> Result<Vec<String>, String> {
        let resp = self
            .client
            .get(self.url("/api/tags"))
            .send()
            .map_err(|e| e.to_string())?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("ollama /api/tags returned {status}"));
        }
        let v: Value = resp.json().map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        if let Some(models) = v.get("models").and_then(|x| x.as_array()) {
            for m in models {
                if let Some(name) = m.get("name").and_then(|x| x.as_str()) {
                    out.push(name.to_string());
                }
            }
        }
        Ok(out)
    }

    pub fn running_models(&self) -> Result<Vec<String>, String> {
        let resp = self
            .client
            .get(self.url("/api/ps"))
            .send()
            .map_err(|e| e.to_string())?;
        let status = resp.status();
        if !status.is_success() {
            return Ok(Vec::new());
        }
        let v: Value = resp.json().map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        if let Some(models) = v.get("models").and_then(|x| x.as_array()) {
            for m in models {
                if let Some(name) = m.get("name").and_then(|x| x.as_str()) {
                    out.push(name.to_string());
                }
            }
        }
        Ok(out)
    }

    pub fn unload(&self, model: &str) {
        let _ = self
            .client
            .post(self.url("/api/generate"))
            .json(&json!({"model": model, "keep_alive": 0, "prompt": ""}))
            .send();
    }
}