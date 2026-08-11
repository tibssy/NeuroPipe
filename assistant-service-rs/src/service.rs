use std::{
    collections::{HashMap, HashSet},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::{json, Value};

use crate::{
    config::Config,
    markdown::{next_sentence, strip_markdown},
    memory::MemoryStore,
    ollama::OllamaClient,
    tools::ToolManager,
};

const MAX_TOOL_ROUNDS: usize = 3;
const MAX_BATCH_SENTENCES: usize = 3;
const MAX_BATCH_CHARS: usize = 150;

#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub role: String,
    pub content: String,
    pub tool_name: Option<String>,
}

struct TtsEventData {
    started_sentences: Vec<String>,
    completed_sentences: usize,
    interrupted: bool,
    active_sentences: usize,
}

pub struct TtsEvents {
    data: Mutex<TtsEventData>,
    changed: Condvar,
}

impl TtsEvents {
    fn new() -> Self {
        Self {
            data: Mutex::new(TtsEventData {
                started_sentences: Vec::new(),
                completed_sentences: 0,
                interrupted: false,
                active_sentences: 0,
            }),
            changed: Condvar::new(),
        }
    }

    fn begin_response(&self) {
        let mut data = self.data.lock().unwrap();
        data.started_sentences.clear();
        data.completed_sentences = 0;
        data.interrupted = false;
        data.active_sentences = 0;
    }

    fn record(&self, message: &Value) {
        let event = message.get("event").and_then(Value::as_str).unwrap_or("");
        let mut data = self.data.lock().unwrap();
        match event {
            "speaking" => {
                if let Some(sentence) = message.get("sentence").and_then(Value::as_str) {
                    data.started_sentences.push(sentence.to_string());
                }
                data.active_sentences += 1;
            }
            "sentence_done" => {
                data.completed_sentences += 1;
                data.active_sentences = data.active_sentences.saturating_sub(1);
            }
            "interrupted" => {
                data.completed_sentences += 1;
                data.interrupted = true;
                data.active_sentences = data.active_sentences.saturating_sub(1);
            }
            _ => return,
        }
        self.changed.notify_all();
    }

    fn started_sentences(&self) -> Vec<String> {
        self.data.lock().unwrap().started_sentences.clone()
    }

    pub fn is_speaking(&self) -> bool {
        self.data.lock().unwrap().active_sentences > 0
    }

    fn wait_for_completion(&self, expected: usize, cancel: &AtomicBool, mode: &Mutex<String>) {
        let mut data = self.data.lock().unwrap();
        while data.completed_sentences < expected
            && !cancel.load(Ordering::SeqCst)
            && *mode.lock().unwrap() == "MODE1"
        {
            let (next, _) = self
                .changed
                .wait_timeout(data, Duration::from_millis(100))
                .unwrap();
            data = next;
        }
    }
}

/// All mutable state, behind interior locks so it can be shared across threads
/// as `Arc<Shared>`.
pub struct Shared {
    pub cfg: Arc<Config>,
    pub mode: Mutex<String>,
    pub model: Mutex<String>,
    pub cancel: AtomicBool,
    pub busy: Mutex<bool>,
    pub history: Mutex<Vec<HistoryEntry>>,
    spoken_buffer: Mutex<Vec<String>>,
    pending_sentences: Mutex<usize>,
    last_activity: Mutex<Instant>,
    last_memory_digest: Mutex<Option<String>>,
    pub tools: Mutex<ToolManager>,
    memory: MemoryStore,
    pub ollama: OllamaClient,
    pub tts_events: Arc<TtsEvents>,
    /// Player name and the volume it was set to before ducking. None = not ducked.
    duck_state: Mutex<Option<(String, f32)>>,
}

pub fn notify(title: &str, body: &str) {
    let _ = Command::new("notify-send")
        .args(["-h", "boolean:transient:true", title, body])
        .output();
}

impl Shared {
    pub fn new(cfg: Config) -> Self {
        let memory = MemoryStore::new(
            "~/.local/share/neuropipe/memory/qdrant",
            &cfg.assistant.memory.collection,
            &cfg.assistant.memory.embedding_model,
        );
        Self {
            model: Mutex::new(cfg.assistant.default_model.clone()),
            tools: Mutex::new({
                let mut t = ToolManager::new(&cfg.assistant.tools);
                t.discover();
                t
            }),
            memory,
            ollama: OllamaClient::new(),
            tts_events: Arc::new(TtsEvents::new()),
            cfg: Arc::new(cfg),
            mode: Mutex::new("IDLE".to_string()),
            cancel: AtomicBool::new(false),
            busy: Mutex::new(false),
            history: Mutex::new(Vec::new()),
            spoken_buffer: Mutex::new(Vec::new()),
            pending_sentences: Mutex::new(0),
            last_activity: Mutex::new(Instant::now()),
            last_memory_digest: Mutex::new(None),
            duck_state: Mutex::new(None),
        }
    }

    pub fn is_busy(&self) -> bool {
        *self.busy.lock().unwrap()
    }

    pub fn push_history(&self, entry: HistoryEntry) {
        self.history.lock().unwrap().push(entry);
    }

    pub fn history_json(&self) -> Vec<Value> {
        self.history
            .lock()
            .unwrap()
            .iter()
            .map(|e| match e.role.as_str() {
                "tool" => json!({
                    "role": "tool",
                    "tool_name": e.tool_name.clone().unwrap_or_else(|| "tool".to_string()),
                    "content": e.content,
                }),
                _ => json!({"role": e.role, "content": e.content}),
            })
            .collect()
    }

    pub fn warm_tts(&self) {
        let addr = self.cfg.ipc.tts_cmd.clone();
        let _ = tts_req(&addr, &json!({"command": "warm"}));
    }

    pub fn list_models(&self) -> Vec<String> {
        self.ollama.list_models().unwrap_or_default()
    }

    pub fn build_system_message(&self, include_tools: bool) -> HistoryEntry {
        let mut parts = vec![self.cfg.assistant.system_prompt.clone()];
        if include_tools {
            let tools = self.tools.lock().unwrap();
            let defs = tools.active_definitions();
            if !defs.is_empty() {
                parts.push(String::new());
                parts.push("You have access to these tools:".to_string());
                for t in &defs {
                    let name = t.pointer("/function/name").and_then(|v| v.as_str()).unwrap_or("");
                    let desc = t
                        .pointer("/function/description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    parts.push(format!("- {name}: {desc}"));
                }
                parts.push(String::new());
                parts.push(self.cfg.assistant.tool_usage_policy.clone());
                parts.push(
                    "Search the web at most once per unique query — report what you find and do \
                     not re-search the same topic with slightly different queries."
                        .to_string(),
                );
            }
        }
        HistoryEntry {
            role: "system".to_string(),
            content: parts.join("\n"),
            tool_name: None,
        }
    }
}

// ---------- ZMQ helpers (ephemeral sockets, like the CLI) ----------

fn stt_set_mode(addr: &str, mode: &str) {
    let ctx = zmq::Context::new();
    if let Ok(sock) = ctx.socket(zmq::REQ) {
        sock.set_linger(0).ok();
        sock.set_rcvtimeo(5000).ok();
        if sock.connect(addr).is_ok() {
            let _ = sock.send(
                &serde_json::to_vec(&json!({"command": "set_mode", "mode": mode})).unwrap(),
                0,
            );
            let _ = sock.recv_multipart(0);
        }
    }
}

pub fn tts_req(addr: &str, cmd: &Value) -> Value {
    let ctx = zmq::Context::new();
    if let Ok(sock) = ctx.socket(zmq::REQ) {
        sock.set_linger(0).ok();
        sock.set_rcvtimeo(5000).ok();
        if sock.connect(addr).is_ok() {
            if sock.send(&serde_json::to_vec(cmd).unwrap(), 0).is_ok() {
                if let Ok(b) = sock.recv_bytes(0) {
                    return serde_json::from_slice(&b).unwrap_or(Value::Null);
                }
            }
        }
    }
    Value::Null
}

fn tts_sub(addr: &str) -> Option<zmq::Socket> {
    let ctx = zmq::Context::new();
    let sock = ctx.socket(zmq::SUB).ok()?;
    sock.connect(addr).ok()?;
    sock.set_subscribe(b"").ok()?;
    Some(sock)
}

pub fn start_tts_event_listener(shared: &Arc<Shared>) {
    let events = Arc::clone(&shared.tts_events);
    let addr = shared.cfg.ipc.tts_events.clone();
    thread::spawn(move || loop {
        let Some(socket) = tts_sub(&addr) else {
            thread::sleep(Duration::from_secs(1));
            continue;
        };
        loop {
            match socket.recv_bytes(0) {
                Ok(bytes) => {
                    if let Ok(message) = serde_json::from_slice::<Value>(&bytes) {
                        events.record(&message);
                    }
                }
                Err(_) => break,
            }
        }
        thread::sleep(Duration::from_secs(1));
    });
}

fn is_cloud_model(model: &str) -> bool {
    model.ends_with(":cloud")
}

// ---------- media ducking helpers ----------

/// Name of the first MPRIS player that is currently Playing, if any.
fn first_playing_player() -> Option<String> {
    let list = Command::new("playerctl").arg("-l").output().ok()?;
    let text = String::from_utf8_lossy(&list.stdout);
    for line in text.lines() {
        let player = line.trim();
        if player.is_empty() {
            continue;
        }
        let Ok(out) = Command::new("playerctl")
            .args(["-p", player, "status"])
            .output()
        else {
            continue;
        };
        if String::from_utf8_lossy(&out.stdout).trim().eq_ignore_ascii_case("playing") {
            return Some(player.to_string());
        }
    }
    None
}

fn playerctl_volume(player: &str) -> Option<f32> {
    let out = Command::new("playerctl")
        .args(["-p", player, "volume"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f32>()
        .ok()
}

fn playerctl_set_volume(player: &str, volume: f32) {
    let _ = Command::new("playerctl")
        .args(["-p", player, "volume", &volume.to_string()])
        .output();
}

/// Current local time as a compact human-readable string, e.g. "Tue 2026-08-09 14:32".
fn now_str() -> String {
    chrono::Local::now().format("%a %Y-%m-%d %H:%M").to_string()
}

// ---------- memory ----------

impl Shared {
    fn memory_allowed_for_model(&self, model: &str) -> bool {
        if is_cloud_model(model) {
            self.cfg.assistant.memory.enabled_cloud
        } else {
            self.cfg.assistant.memory.enabled_local
        }
    }

    fn build_session_transcript(&self) -> String {
        let history = self.history.lock().unwrap();
        let mut lines = Vec::new();
        for entry in history.iter().skip(1) {
            let content = entry.content.trim();
            if content.is_empty() {
                continue;
            }
            if entry.role == "tool" {
                lines.push(format!(
                    "[tool:{}] {content}",
                    entry.tool_name.as_deref().unwrap_or("tool")
                ));
            } else {
                lines.push(format!("[{}] {content}", entry.role));
            }
        }
        lines.join("\n")
    }

    fn fallback_summary(&self, transcript: &str) -> String {
        let chunks: Vec<&str> = transcript
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        let summary = chunks.join(" ");
        let limit = self.cfg.assistant.memory.max_summary_chars;
        if summary.chars().count() <= limit {
            summary
        } else {
            let mut out: String = summary.chars().take(limit.saturating_sub(3)).collect();
            out.push_str("...");
            out
        }
    }

    fn summarize_history_for_memory(&self) -> String {
        let transcript = self.build_session_transcript();
        if transcript.is_empty() {
            return String::new();
        }
        let limit = self.cfg.assistant.memory.max_summary_chars;
        let prompt = format!(
            "Summarize this conversation into compact long-term memory notes. Keep only durable \
             facts, explicit preferences, stable context, and useful follow-ups. Do not include \
             filler, greetings, or tool errors unless they matter to future help. Write plain \
             text, short bullet-style sentences, max {limit} characters."
        );
        let msgs = vec![
            json!({"role": "system", "content": prompt}),
            json!({"role": "user", "content": transcript}),
        ];
        let model = self.model.lock().unwrap().clone();
        match self.ollama.chat_non_stream(&model, &msgs) {
            Ok(content) => {
                if content.is_empty() {
                    self.fallback_summary(&transcript)
                } else if content.chars().count() > limit {
                    let mut out: String = content.chars().take(limit.saturating_sub(3)).collect();
                    out.push_str("...");
                    out
                } else {
                    content
                }
            }
            Err(e) => {
                eprintln!("[memory] LLM summarization failed: {e}");
                self.fallback_summary(&transcript)
            }
        }
    }

    fn sha256(s: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(s.as_bytes());
        format!("{:x}", h.finalize())
    }

    fn maybe_persist_memory(&self, trigger: &str) {
        if trigger == "idle_timeout" && !self.cfg.assistant.memory.summarize_on_idle {
            return;
        }
        if trigger == "stop" && !self.cfg.assistant.memory.summarize_on_stop {
            return;
        }
        let model = self.model.lock().unwrap().clone();
        if !self.memory_allowed_for_model(&model) {
            return;
        }
        let summary = self.summarize_history_for_memory();
        if summary.chars().count() < 24 {
            return;
        }
        let digest = Self::sha256(&summary);
        {
            let last = self.last_memory_digest.lock().unwrap();
            if last.as_deref() == Some(digest.as_str()) {
                return;
            }
        }
        let mut metadata = HashMap::new();
        metadata.insert("trigger".to_string(), trigger.to_string());
        metadata.insert("model".to_string(), model.clone());
        metadata.insert("mode".to_string(), self.mode.lock().unwrap().clone());
        metadata.insert("cloud_model".to_string(), is_cloud_model(&model).to_string());
        // Stamp the stored summary with when it was captured so retrieved
        // memory carries a date. Dedup stays keyed on the raw summary.
        let stored = format!("[{}] {}", now_str(), summary);
        if self.memory.add_summary(&stored, &metadata, &self.ollama) {
            *self.last_memory_digest.lock().unwrap() = Some(digest);
            println!("[memory] Saved summary ({trigger})");
        }
    }

    fn build_memory_context(&self, query: &str) -> Option<String> {
        let model = self.model.lock().unwrap().clone();
        if !self.memory_allowed_for_model(&model) {
            return None;
        }
        let query = query.trim();
        if query.is_empty() {
            return None;
        }
        let top_k = self.cfg.assistant.memory.retrieve_top_k;
        let results = self.memory.search(query, top_k, &self.ollama);
        let mut snippets = Vec::new();
        for (doc, _meta) in results {
            if !doc.trim().is_empty() {
                snippets.push(format!("- {}", doc.trim()));
            }
        }
        if snippets.is_empty() {
            return None;
        }
        Some(
            "Relevant long-term memory from prior sessions. Use it only when helpful and avoid \
             fabricating details.\n"
                .to_string()
                + &snippets.join("\n"),
        )
    }
}

// ---------- TTS / STT ----------

impl Shared {
    fn speak(&self, text: &str) {
        let text = strip_markdown(text);
        if text.trim().is_empty() || self.cancel.load(Ordering::SeqCst) {
            return;
        }
        let addr = self.cfg.ipc.tts_cmd.clone();
        let cmd = json!({"command": "speak", "text": text, "speed": 1.0});
        if !tts_req(&addr, &cmd).is_null() {
            *self.pending_sentences.lock().unwrap() += 1;
            self.spoken_buffer.lock().unwrap().push(text);
        }
    }

    fn stop_tts(&self) -> Value {
        let addr = self.cfg.ipc.tts_cmd.clone();
        tts_req(&addr, &json!({"command": "stop"}))
    }

    fn set_stt_mode(&self, mode: &str) {
        let addr = self.cfg.ipc.stt_cmd.clone();
        stt_set_mode(&addr, mode);
    }

    /// Lower the volume of the currently playing media player so the assistant
    /// can hear the user speak over it. No-op when ducking is disabled, no
    /// player is running, or media is already ducked.
    pub fn duck_media(&self) {
        if !self.cfg.assistant.duck_media {
            return;
        }
        let mut state = self.duck_state.lock().unwrap();
        if state.is_some() {
            return;
        }
        let Some(player) = first_playing_player() else {
            return;
        };
        let volume = playerctl_volume(&player);
        if volume.is_none() {
            return;
        }
        playerctl_set_volume(&player, self.cfg.assistant.duck_volume);
        *state = Some((player, volume.unwrap()));
    }

    /// Restore the media volume that was saved by `duck_media`.
    pub fn unduck_media(&self) {
        let mut state = self.duck_state.lock().unwrap();
        if let Some((player, volume)) = state.take() {
            playerctl_set_volume(&player, volume);
        }
    }

    fn unload_other_models(&self, keep: &str) {
        if let Ok(running) = self.ollama.running_models() {
            for m in running {
                if m != keep {
                    self.ollama.unload(&m);
                }
            }
        }
    }

    fn check_tool_permission(&self, name: &str) -> Option<String> {
        let mut tools = self.tools.lock().unwrap();
        match tools.check(name).as_str() {
            "allow" => None,
            "deny" => Some(format!("Tool '{name}' is disabled.")),
            _ => {
                if tools.is_granted(name) {
                    None
                } else {
                    tools.mark_pending(name);
                    notify(
                        "NeuroPipe Assistant",
                        &format!(
                            "The assistant wants to use '{name}', which requires your permission. \
                             To allow, run: neuro-ipc assistant set-tools '{{\"{name}\": \"allow\"}}'"
                        ),
                    );
                    Some(format!(
                        "Permission needed: '{name}' is set to ask mode. Tell the user to allow it \
                         with set-tools or say 'yes' to grant it for this session."
                    ))
                }
            }
        }
    }

    fn auto_grant_from_text(&self, text: &str) -> bool {
        let lower = text.to_lowercase().trim().to_string();
        let grants = [
            "yes", "yeah", "yep", "sure", "ok", "okay", "go ahead", "allow", "grant", "do it",
            "proceed",
        ];
        if !grants.iter().any(|g| lower == *g || lower.starts_with(g)) {
            return false;
        }
        let mut tools = self.tools.lock().unwrap();
        let Some(name) = tools.pending_tool().map(|s| s.to_string()) else {
            return false;
        };
        if tools.check(&name) == "ask" && !tools.is_granted(&name) {
            tools.grant(&name);
            true
        } else {
            false
        }
    }

    fn truncate_history(&self, spoken: &[String], last_spoken: &str) {
        {
            let mut history = self.history.lock().unwrap();
            while history.last().map(|e| e.role.as_str()) == Some("user") {
                history.pop();
            }
        }
        let content = if !spoken.is_empty() {
            spoken.join(" ")
        } else {
            last_spoken.to_string()
        };
        if !content.is_empty() {
            self.history.lock().unwrap().push(HistoryEntry {
                role: "assistant".to_string(),
                content,
                tool_name: None,
            });
        }
    }
}

// ---------- streaming worker ----------

impl Shared {
    fn stream_and_speak(
        &self,
        tools: Option<&[Value]>,
        memory_context: Option<&str>,
    ) -> Option<(Vec<crate::ollama::ToolCall>, String)> {
        let mut request_messages = self.history_json();
        // Stamp each user message with the time it is being answered, so the
        // model knows "now" (weather tomorrow, upcoming dates, etc.). Applied
        // only to the request payload; stored history stays clean.
        let now = now_str();
        for msg in request_messages.iter_mut() {
            if msg.get("role").and_then(|r| r.as_str()) == Some("user") {
                if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                    msg["content"] = json!(format!("[{now}] {content}"));
                }
            }
        }
        if let Some(ctx) = memory_context {
            let memory_msg = json!({"role": "system", "content": ctx});
            if request_messages.first().and_then(|m| m.get("role")).and_then(|r| r.as_str()) == Some("system") {
                request_messages.insert(1, memory_msg);
            } else {
                request_messages.insert(0, memory_msg);
            }
        }
        let model_name = self.model.lock().unwrap().clone();
        let mut stream = match self.ollama.chat_stream(&model_name, &request_messages, tools) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[Ollama Error: {e}]");
                *self.last_activity.lock().unwrap() = Instant::now();
                return None;
            }
        };

        let mut full = String::new();
        let mut s_buf = String::new();
        let mut called_tools: Vec<crate::ollama::ToolCall> = Vec::new();
        let mode_is_two = *self.mode.lock().unwrap() == "MODE2";
        let mut is_first = true;
        let mut batch: Vec<String> = Vec::new();
        let mut batch_chars = 0usize;

        loop {
            if self.cancel.load(Ordering::SeqCst) {
                break;
            }
            match stream.next_chunk() {
                Ok(Some(chunk)) => {
                    if let Some(content) = chunk.content {
                        print!("{content}");
                        use std::io::Write;
                        let _ = std::io::stdout().flush();
                        full.push_str(&content);
                        s_buf.push_str(&content);
                        loop {
                            match next_sentence(&s_buf) {
                                Some((sentence, offset)) => {
                                    s_buf = s_buf[offset..].to_string();
                                    if mode_is_two {
                                        if is_first {
                                            self.speak(&sentence);
                                            is_first = false;
                                        } else {
                                            batch.push(sentence.clone());
                                            batch_chars += sentence.len();
                                            if batch.len() >= MAX_BATCH_SENTENCES
                                                || batch_chars >= MAX_BATCH_CHARS
                                            {
                                                self.speak(&batch.join(" "));
                                                batch.clear();
                                                batch_chars = 0;
                                            }
                                        }
                                    } else {
                                        self.speak(&sentence);
                                    }
                                }
                                None => break,
                            }
                        }
                    }
                    called_tools.extend(chunk.tool_calls);
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("\n[Ollama Error: {e}]");
                    *self.last_activity.lock().unwrap() = Instant::now();
                    return None;
                }
            }
        }

        if self.cancel.load(Ordering::SeqCst) {
            println!("\n[Interrupted]\n");
            return None;
        }

        println!("\n");
        let remaining = s_buf.trim().to_string();
        if !remaining.is_empty() {
            if mode_is_two && !batch.is_empty() {
                batch.push(remaining);
                self.speak(&batch.join(" "));
            } else {
                self.speak(&remaining);
            }
        } else if mode_is_two && !batch.is_empty() {
            self.speak(&batch.join(" "));
        }

        if !called_tools.is_empty() {
            *self.last_activity.lock().unwrap() = Instant::now();
            return Some((called_tools, full));
        }

        self.history.lock().unwrap().push(HistoryEntry {
            role: "assistant".to_string(),
            content: full.clone(),
            tool_name: None,
        });
        *self.last_activity.lock().unwrap() = Instant::now();
        None
    }

    fn ask_ollama(&self, text: &str) {
        println!("\nYou: {text}");
        if self.auto_grant_from_text(text) {
            println!("[Auto-granted permission for this session]");
        }
        self.history.lock().unwrap().push(HistoryEntry {
            role: "user".to_string(),
            content: text.to_string(),
            tool_name: None,
        });

        let memory_context = self.build_memory_context(text);
        let tools_payload = self.tools.lock().unwrap().active_definitions();
        let tools: Option<Vec<Value>> = if tools_payload.is_empty() {
            None
        } else {
            Some(tools_payload)
        };

        // Small local models sometimes emit the same tool call twice (within one
        // response or repeated in a later round); execute each unique call once.
        let mut executed_tools: HashSet<(String, String)> = HashSet::new();

        for round_num in 0..MAX_TOOL_ROUNDS {
            let result = self.stream_and_speak(
                tools.as_deref(),
                if round_num == 0 { memory_context.as_deref() } else { None },
            );
            let (called, spoken) = match result {
                Some(r) => r,
                None => return,
            };
            if !spoken.trim().is_empty() {
                self.history.lock().unwrap().push(HistoryEntry {
                    role: "assistant".to_string(),
                    content: spoken,
                    tool_name: None,
                });
            }
            if called.is_empty() {
                return;
            }
            for tc in called {
                let name = tc.name.clone();
                let args = tc.arguments.clone();
                let key = (name.clone(), serde_json::to_string(&args).unwrap_or_default());
                if !executed_tools.insert(key) {
                    println!("\n[Tool: {name} — skipped duplicate call]");
                    continue;
                }
                println!("\n[Tool: {name}({args})]");
                match self.check_tool_permission(&name) {
                    Some(err) => {
                        println!("[Permission denied: {err}]");
                        self.history.lock().unwrap().push(HistoryEntry {
                            role: "tool".to_string(),
                            content: format!("Error: {err}"),
                            tool_name: Some(name),
                        });
                    }
                    None => {
                        let result = self.tools.lock().unwrap().execute(&name, &args);
                        println!("[Result: {}]", &result[..std::cmp::min(result.len(), 200)]);
                        self.history.lock().unwrap().push(HistoryEntry {
                            role: "tool".to_string(),
                            content: result,
                            tool_name: Some(name),
                        });
                    }
                }
            }
        }

        self.history.lock().unwrap().push(HistoryEntry {
            role: "user".to_string(),
            content: "Tell the user you searched but could not find a clear answer to their question."
                .to_string(),
            tool_name: None,
        });
        if let Some((_, final_spoken)) = self.stream_and_speak(None, None) {
            if !final_spoken.trim().is_empty() {
                self.history.lock().unwrap().push(HistoryEntry {
                    role: "assistant".to_string(),
                    content: final_spoken,
                    tool_name: None,
                });
            }
        }
        println!("\n[Max tool rounds reached]");
    }

    pub fn process_and_respond(&self, text: String) {
        *self.pending_sentences.lock().unwrap() = 0;
        self.spoken_buffer.lock().unwrap().clear();

        let mode = self.mode.lock().unwrap().clone();
        self.tts_events.begin_response();
        if mode == "MODE1" {
            self.set_stt_mode("IDLE");
        }

        self.ask_ollama(&text);

        if mode == "MODE1" {
            let remaining = *self.pending_sentences.lock().unwrap();
            self.tts_events
                .wait_for_completion(remaining, &self.cancel, &self.mode);
            if *self.mode.lock().unwrap() == "MODE1" {
                self.set_stt_mode("VAD");
            }
        }
    }

    pub fn stop(&self) {
        if *self.mode.lock().unwrap() == "IDLE" && !self.is_busy() {
            return;
        }
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.stop_tts();
        self.unduck_media();
        if self.is_busy() {
            let _ = self.interrupt();
        }
        self.maybe_persist_memory("stop");
        self.set_stt_mode("IDLE");
        self.tools.lock().unwrap().reset_session();
        *self.history.lock().unwrap() = vec![self.build_system_message(false)];
        *self.mode.lock().unwrap() = "IDLE".to_string();
        notify("NeuroPipe", "Idle");
    }

    pub fn interrupt(&self) -> String {
        if !self.is_busy() && !self.tts_events.is_speaking() {
            return String::new();
        }
        self.cancel.store(true, Ordering::SeqCst);
        let reply = self.stop_tts();
        let last_sentence = reply
            .get("last_sentence")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let started = self.tts_events.started_sentences();
        let spoken = if started.is_empty() {
            self.spoken_buffer.lock().unwrap().clone()
        } else {
            started
        };
        self.truncate_history(&spoken, &last_sentence);
        *self.pending_sentences.lock().unwrap() = 0;
        self.tts_events.changed.notify_all();
        if *self.mode.lock().unwrap() == "MODE1" {
            self.set_stt_mode("VAD");
        }
        last_sentence
    }

    pub fn get_tts_state(&self) -> Value {
        let addr = self.cfg.ipc.tts_cmd.clone();
        tts_req(&addr, &json!({"command": "get_state"}))
    }

    pub fn reset_memory(&self) -> Value {
        let mut memory = MemoryStore::new(
            "~/.local/share/neuropipe/memory/qdrant",
            &self.cfg.assistant.memory.collection,
            &self.cfg.assistant.memory.embedding_model,
        );
        let (status, deleted) = memory.reset();
        json!({"status": status, "deleted": deleted})
    }

    pub fn start_session(
        &self,
        mode: &str,
        model: Option<String>,
        engine: Option<String>,
        voice: Option<String>,
    ) {
        if let Some(m) = model {
            *self.model.lock().unwrap() = m.clone();
            self.unload_other_models(&m);
        }
        let idle = self.last_activity.lock().unwrap().elapsed()
            > Duration::from_secs(self.cfg.assistant.history_idle_timeout_sec);
        if idle {
            println!("Idle > 1h, clearing history.");
            self.maybe_persist_memory("idle_timeout");
        }
        self.tools.lock().unwrap().reset_session();
        {
            let mut history = self.history.lock().unwrap();
            if idle || history.len() <= 1 {
                *history = vec![self.build_system_message(true)];
            }
        }

        let mut tts_state = serde_json::Map::new();
        tts_state.insert("command".to_string(), json!("set_state"));
        if let Some(engine) = engine {
            tts_state.insert("engine".to_string(), json!(engine));
        }
        if let Some(voice) = voice {
            tts_state.insert("voice".to_string(), json!(voice));
        }
        if tts_state.len() > 1 {
            let addr = self.cfg.ipc.tts_cmd.clone();
            let _ = tts_req(&addr, &Value::Object(tts_state));
        }

        *self.mode.lock().unwrap() = mode.to_string();
        self.set_stt_mode("VAD");
        notify("NeuroPipe", "Listening");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        let mut cfg = Config::load();
        cfg.assistant.duck_media = true;
        cfg.assistant.duck_volume = 0.1;
        cfg
    }

    /// Requires an MPRIS player actually playing (e.g. `mpv`). Skips when
    /// playerctl is missing or no player is running.
    fn playing_player() -> Option<String> {
        if !first_playing_player().is_some() {
            return None;
        }
        first_playing_player()
    }

    #[test]
    fn duck_then_unduck_restores_volume() {
        let Some(player) = playing_player() else {
            eprintln!("skipping: no playing MPRIS player available");
            return;
        };
        let original = playerctl_volume(&player).unwrap_or(1.0);
        let shared = Shared::new(test_config());
        shared.duck_media();
        let ducked = playerctl_volume(&player).unwrap_or(-1.0);
        assert!(
            (ducked - 0.1).abs() < 0.01,
            "expected ducked volume 0.1, got {ducked}"
        );
        shared.unduck_media();
        let restored = playerctl_volume(&player).unwrap_or(-1.0);
        assert!(
            (restored - original).abs() < 0.01,
            "expected restored volume {original}, got {restored}"
        );
    }

    #[test]
    fn duck_is_idempotent_until_unduck() {
        let Some(player) = playing_player() else {
            eprintln!("skipping: no playing MPRIS player available");
            return;
        };
        let original = playerctl_volume(&player).unwrap_or(1.0);
        let shared = Shared::new(test_config());
        shared.duck_media();
        shared.duck_media();
        let ducked = playerctl_volume(&player).unwrap_or(-1.0);
        assert!((ducked - 0.1).abs() < 0.01);
        shared.unduck_media();
        shared.unduck_media();
        let restored = playerctl_volume(&player).unwrap_or(-1.0);
        assert!((restored - original).abs() < 0.01);
    }
}
