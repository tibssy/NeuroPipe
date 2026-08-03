use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::ollama::OllamaClient;

#[derive(Serialize, Deserialize, Clone)]
struct Entry {
    id: String,
    vector: Vec<f32>,
    document: String,
    metadata: std::collections::HashMap<String, String>,
}

pub struct MemoryStore {
    path: PathBuf,
    embedding_model: String,
    entries: Mutex<Vec<Entry>>,
    disabled: Mutex<Option<String>>,
}

fn expand(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest)
    } else {
        PathBuf::from(path)
    }
}

impl MemoryStore {
    pub fn new(path: &str, collection: &str, embedding_model: &str) -> Self {
        // Fresh native store: a json file under the same parent dir, named
        // after the configured collection.
        let dir = expand(path);
        let parent = dir.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let file = parent.join(format!("{collection}.json"));
        let mut entries = Vec::new();
        if file.exists() {
            if let Ok(text) = fs::read_to_string(&file) {
                if let Ok(list) = serde_json::from_str::<Vec<Entry>>(&text) {
                    entries = list;
                }
            }
        }
        Self {
            path: file,
            embedding_model: embedding_model.to_string(),
            entries: Mutex::new(entries),
            disabled: Mutex::new(None),
        }
    }

    fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let entries = self.entries.lock().unwrap();
        let text = serde_json::to_string(&*entries).map_err(|e| e.to_string())?;
        fs::write(&self.path, text).map_err(|e| e.to_string())
    }

    pub fn add_summary(
        &self,
        summary: &str,
        metadata: &std::collections::HashMap<String, String>,
        ollama: &OllamaClient,
    ) -> bool {
        if self.disabled.lock().unwrap().is_some() {
            return false;
        }
        let Ok(embed) = ollama.embed(&self.embedding_model, &[summary.to_string()]) else {
            *self.disabled.lock().unwrap() = Some("embedding failed".to_string());
            return false;
        };
        let Some(vector) = embed.into_iter().next() else {
            return false;
        };
        let mut meta = metadata.clone();
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        meta.insert("saved_at".to_string(), ts.to_string());

        let entry = Entry {
            id: uuid::Uuid::new_v4().to_string(),
            vector,
            document: summary.to_string(),
            metadata: meta,
        };
        self.entries.lock().unwrap().push(entry);
        self.save().is_ok()
    }

    pub fn search(
        &self,
        query: &str,
        top_k: usize,
        ollama: &OllamaClient,
    ) -> Vec<(String, serde_json::Value)> {
        if self.disabled.lock().unwrap().is_some() || top_k == 0 {
            return Vec::new();
        }
        let Ok(embed) = ollama.embed(&self.embedding_model, &[query.to_string()]) else {
            return Vec::new();
        };
        let Some(qv) = embed.into_iter().next() else {
            return Vec::new();
        };
        let entries = self.entries.lock().unwrap();
        let mut scored: Vec<(f32, &Entry)> = entries
            .iter()
            .map(|e| (cosine(&qv, &e.vector), e))
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored
            .into_iter()
            .take(top_k)
            .filter(|(s, _)| *s > 0.0)
            .map(|(_, e)| {
                let document = e.document.clone();
                let metadata =
                    serde_json::to_value(&e.metadata).unwrap_or_else(|_| serde_json::json!({}));
                (document, metadata)
            })
            .collect()
    }

    pub fn reset(&mut self) -> (String, usize) {
        let n = self.entries.lock().unwrap().len();
        self.entries.lock().unwrap().clear();
        if self.save().is_err() {
            return ("disabled".to_string(), 0);
        }
        ("ok".to_string(), n)
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0;
    let mut na = 0.0;
    let mut nb = 0.0;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}