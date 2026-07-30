import zmq
import re
import os
import time
import hashlib
import threading
import httpx
import subprocess as sp
import argparse
from ollama import Client

from tool_manager import ToolManager
from neuropipe_config import load_config
from memory_store import MemoryStore

OLLAMA_SOCK = "/tmp/ollama.sock"
if os.path.exists(OLLAMA_SOCK):
    _transport = httpx.HTTPTransport(uds=OLLAMA_SOCK)
    _ollama = Client(transport=_transport)
else:
    _ollama = Client()

def chat(*args, **kwargs):
    return _ollama.chat(*args, **kwargs)


def embed_texts(model: str, texts: list[str]) -> list[list[float]]:
    response = _ollama.embed(model=model, input=texts)
    vectors = getattr(response, "embeddings", None)
    if vectors is None and isinstance(response, dict):
        vectors = response.get("embeddings")
    if not isinstance(vectors, list):
        raise RuntimeError("Ollama embed response did not contain embeddings")
    return vectors

SENTENCE_END = re.compile(r'[.!?](?:\s|$)')

_CONFIG = load_config()

CMD_ADDR = _CONFIG["ipc"]["assistant_cmd"]
STT_PUB_ADDR = _CONFIG["ipc"]["stt_pub"]
STT_CMD_ADDR = _CONFIG["ipc"]["stt_cmd"]
TTS_CMD_ADDR = _CONFIG["ipc"]["tts_cmd"]
TTS_EVENTS_ADDR = _CONFIG["ipc"]["tts_events"]

DEFAULT_MODEL = _CONFIG["assistant"]["default_model"]

def _build_system_message(tools: list[dict], instructions: dict) -> dict:
    parts = [
        instructions["system_prompt"],
    ]
    if tools:
        descs = [f"- {t['function']['name']}: {t['function']['description']}" for t in tools]
        parts.append("")
        parts.append("You have access to these tools:")
        parts.extend(descs)
        parts.append("")
        parts.append(instructions["tool_usage_policy"])
    return {'role': 'system', 'content': "\n".join(parts)}


class AssistantService:
    def __init__(self):
        self.ctx = zmq.Context()
        self.config = _CONFIG

        self.cmd_socket = self.ctx.socket(zmq.REP)
        self.cmd_socket.bind(CMD_ADDR)

        self.stt_sub = self.ctx.socket(zmq.SUB)
        self.stt_sub.connect(STT_PUB_ADDR)
        self.stt_sub.setsockopt_string(zmq.SUBSCRIBE, "")

        # Persistent STT REQ socket
        self.stt_cmd_sock = self.ctx.socket(zmq.REQ)
        self.stt_cmd_sock.setsockopt(zmq.RCVTIMEO, 5000)
        self.stt_cmd_sock.setsockopt(zmq.LINGER, 0)
        self.stt_cmd_sock.connect(STT_CMD_ADDR)
        self.stt_lock = threading.Lock()

        # Persistent TTS REQ socket
        self.tts_cmd_sock = self.ctx.socket(zmq.REQ)
        try:
            self.tts_cmd_sock.setsockopt(zmq.REQ_RELAXED, 1)
            self.tts_cmd_sock.setsockopt(zmq.REQ_CORRELATE, 1)
        except AttributeError:
            pass
        self.tts_cmd_sock.setsockopt(zmq.RCVTIMEO, 5000)
        self.tts_cmd_sock.setsockopt(zmq.LINGER, 0)
        self.tts_cmd_sock.connect(TTS_CMD_ADDR)
        self.tts_lock = threading.Lock()

        self.mode = "IDLE"
        self.ollama_model = DEFAULT_MODEL

        self.cancel_event = threading.Event()
        self.ollama_thread = None
        self.history = [_build_system_message([], self.config["assistant"]["instructions"])]
        self.last_activity = time.time()
        self._pending_sentences = 0
        self._spoken_buffer = []
        self._spoken_lock = threading.Lock()
        self.memory_config = self.config["assistant"]["memory"]
        self.memory_store = MemoryStore(
            self.memory_config["qdrant_path"],
            self.memory_config["collection"],
            self.memory_config["embedding_model"],
            embed_texts,
        )
        self.last_memory_digest = ""

        # External tool plugins
        self.tool_manager = ToolManager(initial_config=self.config["assistant"].get("tools", {}))
        self.tool_manager.discover()

        # Persistent TTS events SUB socket
        self.tts_events_sock = self.ctx.socket(zmq.SUB)
        self.tts_events_sock.connect(TTS_EVENTS_ADDR)
        self.tts_events_sock.setsockopt_string(zmq.SUBSCRIBE, "")

        sp.run(["notify-send", "-h", "boolean:transient:true", "NeuroPipe", "Starting..."], capture_output=True)

    def set_stt_mode(self, mode):
        with self.stt_lock:
            self.stt_cmd_sock.send_json({"command": "set_mode", "mode": mode})
            return self.stt_cmd_sock.recv_json()

    def send_tts_command(self, cmd_dict):
        with self.tts_lock:
            self.tts_cmd_sock.send_json(cmd_dict)
            return self.tts_cmd_sock.recv_json()

    def stop_tts(self):
        try:
            return self.send_tts_command({"command": "stop"})
        except zmq.ZMQError as e:
            print(f"stop_tts error: {e}")
            return {}

    def _strip_markdown(self, text: str) -> str:
        s = text
        s = re.sub(r'\[([^\]]*)\]\([^)]+\)', r'\1', s)  # [text](url) -> text
        s = re.sub(r'!\[([^\]]*)\]\([^)]+\)', r'\1', s)  # ![alt](url) -> alt
        s = re.sub(r'```[\s\S]*?```', '', s)  # code blocks
        s = re.sub(r'`([^`]+)`', r'\1', s)  # inline code
        s = re.sub(r'\*\*([^*]+)\*\*', r'\1', s)  # **bold**
        s = re.sub(r'\*([^*]+)\*', r'\1', s)  # *italic*
        s = re.sub(r'__([^_]+)__', r'\1', s)  # __bold__ (greedy would break this)
        s = re.sub(r'(?<!\w)_([^_]+)_(?!\w)', r'\1', s)  # _italic_ (word boundaries)
        s = re.sub(r'~~([^~]+)~~', r'\1', s)  # ~~strikethrough~~
        s = re.sub(r'^#+\s*', '', s, flags=re.MULTILINE)  # # headings
        s = re.sub(r'^>\s*', '', s, flags=re.MULTILINE)  # > blockquotes
        s = re.sub(r'^[-*+]\s+', '', s, flags=re.MULTILINE)  # - list items
        s = re.sub(r'^\d+\.\s+', '', s, flags=re.MULTILINE)  # 1. list items
        s = re.sub(r'^[-*_]{3,}\s*$', '', s, flags=re.MULTILINE)  # hr --- *** ___
        s = re.sub(r'<[^>]+>', '', s)  # <html> or <url>
        return s.strip()

    def speak(self, text):
        text = self._strip_markdown(text)
        if not text.strip() or self.cancel_event.is_set():
            return None
        cmd = {"command": "speak", "text": text, "speed": 1.0}
        try:
            reply = self.send_tts_command(cmd)
            self._pending_sentences += 1
            with self._spoken_lock:
                self._spoken_buffer.append(text)
            return reply
        except zmq.ZMQError as e:
            print(f"speak error: {e}")
            return None

    def _truncate_history(self, spoken_sentences, last_spoken=""):
        while self.history and self.history[-1]['role'] == 'user':
            self.history.pop()
        content = " ".join(spoken_sentences)
        if not content:
            content = last_spoken
        if content:
            self.history.append({'role': 'assistant', 'content': content})

    def _is_cloud_model(self, model: str) -> bool:
        return isinstance(model, str) and model.endswith(":cloud")

    def _memory_allowed_for_model(self, model: str) -> bool:
        if self._is_cloud_model(model):
            return self.memory_config["enabled_cloud"]
        return self.memory_config["enabled_local"]

    def _memory_allowed(self) -> bool:
        return self._memory_allowed_for_model(self.ollama_model)

    def _build_session_transcript(self) -> str:
        lines = []
        for entry in self.history[1:]:
            role = entry.get("role", "")
            content = (entry.get("content") or "").strip()
            if not content:
                continue
            if role == "tool":
                tool_name = entry.get("tool_name", "tool")
                lines.append(f"[tool:{tool_name}] {content}")
            else:
                lines.append(f"[{role}] {content}")
        return "\n".join(lines)

    def _fallback_summary(self, transcript: str) -> str:
        chunks = []
        for line in transcript.splitlines():
            text = line.strip()
            if text:
                chunks.append(text)
        summary = " ".join(chunks)
        limit = int(self.memory_config["max_summary_chars"])
        if len(summary) <= limit:
            return summary
        return summary[: limit - 3].rstrip() + "..."

    def _summarize_history_for_memory(self) -> str:
        transcript = self._build_session_transcript()
        if not transcript:
            return ""

        limit = int(self.memory_config["max_summary_chars"])
        prompt = (
            "Summarize this conversation into compact long-term memory notes. "
            "Keep only durable facts, explicit preferences, stable context, and useful follow-ups. "
            "Do not include filler, greetings, or tool errors unless they matter to future help. "
            "Write plain text, short bullet-style sentences, max "
            f"{limit} characters."
        )
        try:
            reply = chat(
                model=self.ollama_model,
                messages=[
                    {"role": "system", "content": prompt},
                    {"role": "user", "content": transcript},
                ],
                stream=False,
            )
            message = getattr(reply, "message", None)
            content = getattr(message, "content", "") if message else ""
            if not content and isinstance(reply, dict):
                content = ((reply.get("message") or {}).get("content") or "")
            content = content.strip()
            if not content:
                return self._fallback_summary(transcript)
            if len(content) > limit:
                content = content[: limit - 3].rstrip() + "..."
            return content
        except Exception as e:
            print(f"[memory] LLM summarization failed: {e}")
            return self._fallback_summary(transcript)

    def _maybe_persist_memory(self, trigger: str):
        if trigger == "idle_timeout" and not self.memory_config["summarize_on_idle"]:
            return
        if trigger == "stop" and not self.memory_config["summarize_on_stop"]:
            return
        if not self._memory_allowed():
            return

        summary = self._summarize_history_for_memory()
        if len(summary) < 24:
            return

        digest = hashlib.sha256(summary.encode("utf-8")).hexdigest()
        if digest == self.last_memory_digest:
            return

        try:
            saved = self.memory_store.add_summary(
                summary,
                {
                    "trigger": trigger,
                    "model": self.ollama_model,
                    "mode": self.mode,
                    "cloud_model": str(self._is_cloud_model(self.ollama_model)).lower(),
                },
            )
            if saved:
                self.last_memory_digest = digest
                print(f"[memory] Saved summary ({trigger})")
        except Exception as e:
            print(f"[memory] Failed to save summary: {e}")

    def _build_memory_context(self, query: str) -> str | None:
        if not self._memory_allowed():
            return None

        query = query.strip()
        if not query:
            return None

        try:
            results = self.memory_store.search(query, int(self.memory_config["retrieve_top_k"]))
        except Exception as e:
            print(f"[memory] Failed to search memory: {e}")
            return None

        snippets = []
        for result in results:
            text = (result.get("document") or "").strip()
            if text:
                snippets.append(f"- {text}")

        if not snippets:
            return None

        return (
            "Relevant long-term memory from prior sessions. "
            "Use it only when helpful and avoid fabricating details.\n"
            + "\n".join(snippets)
        )

    def _stream_and_speak(self, tools, round_num=0, memory_context=None):
        full_response = ""
        sentence_buffer = ""
        called_tools = []

        tts_batch_buffer = []
        tts_batch_chars = 0
        MAX_BATCH_SENTENCES = 3
        MAX_BATCH_CHARS = 150
        is_first = True

        request_messages = list(self.history)
        if memory_context:
            memory_msg = {'role': 'system', 'content': memory_context}
            if request_messages and request_messages[0].get('role') == 'system':
                request_messages = [request_messages[0], memory_msg, *request_messages[1:]]
            else:
                request_messages = [memory_msg, *request_messages]

        kwargs = {"model": self.ollama_model, "messages": request_messages, "stream": True}
        if tools and round_num == 0:
            kwargs["tools"] = tools

        try:
            for chunk in chat(**kwargs):
                if self.cancel_event.is_set():
                    break

                if chunk.message.content:
                    content = chunk.message.content
                    print(content, end="", flush=True)
                    full_response += content
                    sentence_buffer += content

                    while True:
                        m = SENTENCE_END.search(sentence_buffer)
                        if not m:
                            break
                        sentence = sentence_buffer[:m.end()].strip()
                        sentence_buffer = sentence_buffer[m.end():]

                        if self.mode == "MODE2":
                            if is_first:
                                self.speak(sentence)
                                is_first = False
                            else:
                                tts_batch_buffer.append(sentence)
                                tts_batch_chars += len(sentence)
                                if len(tts_batch_buffer) >= MAX_BATCH_SENTENCES or tts_batch_chars >= MAX_BATCH_CHARS:
                                    self.speak(" ".join(tts_batch_buffer))
                                    tts_batch_buffer.clear()
                                    tts_batch_chars = 0
                        else:
                            self.speak(sentence)

                if chunk.message.tool_calls:
                    called_tools.extend(chunk.message.tool_calls)
        except Exception as e:
            print(f"\n[Ollama Error: {e}]")
            self.last_activity = time.time()
            return None, ""

        if self.cancel_event.is_set():
            print("\n[Interrupted]\n")
            return None, full_response

        print("\n")
        remaining = sentence_buffer.strip()
        if remaining:
            if self.mode == "MODE2" and tts_batch_buffer:
                tts_batch_buffer.append(remaining)
                self.speak(" ".join(tts_batch_buffer))
            else:
                self.speak(remaining)
        elif self.mode == "MODE2" and tts_batch_buffer:
            self.speak(" ".join(tts_batch_buffer))

        if called_tools:
            return called_tools, full_response

        self.history.append({'role': 'assistant', 'content': full_response})
        self.last_activity = time.time()
        return None, full_response

    def _check_tool_permission(self, name: str) -> str | None:
        level = self.tool_manager.check(name)
        if level == "allow":
            return None
        if level == "deny":
            return f"Tool '{name}' is disabled."
        if self.tool_manager.is_granted(name):
            return None
        sp.run(
            ["notify-send", "-h", "boolean:transient:true",
             "NeuroPipe Assistant",
             f"The assistant wants to use '{name}', which requires your permission. To allow, run: neuro-ipc assistant set-tools '{{\"{name}\": \"allow\"}}'"],
            capture_output=True,
        )
        return f"Permission needed: '{name}' is set to ask mode. Tell the user to allow it with set-tools or say 'yes' to grant it for this session."

    def _auto_grant_from_text(self, text: str):
        lower = text.lower().strip()
        grants = {"yes", "yeah", "yep", "sure", "ok", "okay", "go ahead", "allow", "grant", "do it", "proceed"}
        if lower in grants or any(lower.startswith(w) for w in grants):
            any_granted = False
            for name in self.tool_manager.list_all():
                level = self.tool_manager.check(name)
                if level == "ask" and not self.tool_manager.is_granted(name):
                    self.tool_manager.grant(name)
                    any_granted = True
            return any_granted
        return False

    def ask_ollama(self, text):
        print(f"\nYou: {text}")

        if self._auto_grant_from_text(text):
            print("[Auto-granted permission for all 'ask' tools this session]")

        self.history.append({'role': 'user', 'content': text})
        memory_context = self._build_memory_context(text)

        tools = self.tool_manager.active_definitions() or None

        for round_num in range(3):
            called_tools, spoken = self._stream_and_speak(
                tools,
                round_num,
                memory_context if round_num == 0 else None,
            )
            if called_tools is None:
                return

            if spoken.strip():
                self.history.append({'role': 'assistant', 'content': spoken})

            for tc in called_tools:
                name = tc.function.name
                args = dict(tc.function.arguments or {})
                print(f"\n[Tool: {name}({args})]")
                err = self._check_tool_permission(name)
                if err:
                    print(f"[Permission denied: {err}]")
                    self.history.append({'role': 'tool', 'tool_name': name, 'content': f"Error: {err}"})
                    continue
                result = self.tool_manager.execute(name, args)
                print(f"[Result: {result[:200]}]")
                self.history.append({'role': 'tool', 'tool_name': name, 'content': result})

        self.history.append({'role': 'user', 'content': 'Tell the user you searched but could not find a clear answer to their question.'})
        _, final = self._stream_and_speak(None)
        if final.strip():
            self.history.append({'role': 'assistant', 'content': final})
        print("\n[Max tool rounds reached]")

    def is_busy(self):
        return self.ollama_thread is not None and self.ollama_thread.is_alive()

    def interrupt(self):
        if not self.is_busy():
            return ""
        self.cancel_event.set()
        reply = self.stop_tts()
        last_sentence = reply.get("last_sentence", "") if reply else ""
        self.ollama_thread.join(timeout=5)
        with self._spoken_lock:
            spoken = list(self._spoken_buffer)
        self._truncate_history(spoken, last_sentence)
        self.cancel_event.clear()
        if self.mode == "MODE1":
            self.set_stt_mode("VAD")
        return last_sentence

    def _unload_other_models(self):
        try:
            running = _ollama.ps()
            for m in running.models or []:
                if m.model and m.model != self.ollama_model:
                    _ollama.generate(model=m.model, keep_alive=0, prompt="")
        except Exception:
            pass

    def _warm_tts(self):
        try:
            self.send_tts_command({"command": "warm"})
        except zmq.ZMQError:
            pass

    def start_session(self, mode, model=None, engine=None, voice=None):
        if model:
            self.ollama_model = model
        self._unload_other_models()
        idle = time.time() - self.last_activity > self.config["assistant"]["history_idle_timeout_sec"]
        if idle:
            print("Idle > 1h, clearing history.")
            self._maybe_persist_memory("idle_timeout")
        self.tool_manager.reset_session()
        # Only reset history on first start or idle timeout — preserve on mode switches
        if idle or len(self.history) <= 1:
            tools = self.tool_manager.active_definitions()
            self.history = [_build_system_message(tools, self.config["assistant"]["instructions"])]

        tts_state = {}
        if engine:
            tts_state["engine"] = engine
        if voice:
            tts_state["voice"] = voice
        if tts_state:
            tts_state["command"] = "set_state"
            self.send_tts_command(tts_state)

        self.mode = mode
        self.set_stt_mode("VAD")
        sp.run(["notify-send", "-h", "boolean:transient:true", "NeuroPipe", "Listening"], capture_output=True)

    def stop(self):
        if self.mode == "IDLE" and not self.is_busy():
            return
        self.cancel_event.set()
        self.stop_tts()
        if self.is_busy():
            self.interrupt()
        self._maybe_persist_memory("stop")
        self.set_stt_mode("IDLE")
        self.tool_manager.reset_session()
        self.history = [_build_system_message([], self.config["assistant"]["instructions"])]
        self.mode = "IDLE"
        sp.run(["notify-send", "-h", "boolean:transient:true", "NeuroPipe", "Idle"], capture_output=True)

    def get_history(self):
        return [dict(entry) for entry in self.history]

    def reset_longterm_memory(self):
        result = self.memory_store.reset()
        if result.get("status") == "ok":
            self.last_memory_digest = ""
        return result

    def _process_and_respond(self, text):
        self._pending_sentences = 0
        with self._spoken_lock:
            self._spoken_buffer = []

        if self.mode == "MODE1":
            self.set_stt_mode("IDLE")
            # Drain stale events from persistent socket
            while True:
                try:
                    self.tts_events_sock.recv_json(flags=zmq.NOBLOCK)
                except zmq.Again:
                    break

        self.ask_ollama(text)

        if self.mode == "MODE1":
            remaining = self._pending_sentences
            while remaining > 0 and not self.cancel_event.is_set() and self.mode == "MODE1":
                try:
                    msg = self.tts_events_sock.recv_json(flags=zmq.NOBLOCK)
                    event = msg.get("event")
                    if event in ("sentence_done", "interrupted"):
                        remaining -= 1
                except zmq.Again:
                    time.sleep(0.05)
            if self.mode == "MODE1":
                self.set_stt_mode("VAD")

    def handle_transcription(self, text):
        if self.mode not in ("MODE1", "MODE2"):
            return
        if self.is_busy():
            if self.mode == "MODE1":
                print("\n[Busy — ignoring new input]")
                return
            elif self.mode == "MODE2":
                self.interrupt()

        self.cancel_event.clear()
        self.ollama_thread = threading.Thread(
            target=self._process_and_respond, args=(text,)
        )
        self.ollama_thread.daemon = True
        self.ollama_thread.start()

    def run(self):
        print(f"NeuroPipe is ready.")
        print(f"Model: {self.ollama_model}")
        print(f"Socket: {CMD_ADDR}")

        poller = zmq.Poller()
        poller.register(self.cmd_socket, zmq.POLLIN)
        poller.register(self.stt_sub, zmq.POLLIN)

        try:
            while True:
                socks = dict(poller.poll(timeout=500))

                if self.cmd_socket in socks:
                    msg = self.cmd_socket.recv_json()
                    cmd = msg.get("command")

                    print(f"Command: {cmd}")

                    try:
                        if cmd == "mode1":
                            self.start_session(
                                "MODE1",
                                model=msg.get("model"),
                                engine=msg.get("engine"),
                                voice=msg.get("voice"),
                            )
                            self.cmd_socket.send_json(
                                {"status": "ok", "mode": "MODE1"}
                            )

                        elif cmd == "mode2":
                            self.start_session(
                                "MODE2",
                                model=msg.get("model"),
                                engine=msg.get("engine"),
                                voice=msg.get("voice"),
                            )
                            self.cmd_socket.send_json(
                                {"status": "ok", "mode": "MODE2"}
                            )

                        elif cmd == "interrupt":
                            last = self.interrupt()
                            self.cmd_socket.send_json(
                                {"status": "interrupted", "last_sentence": last}
                            )

                        elif cmd == "stop":
                            self.stop()
                            self.cmd_socket.send_json({"status": "stopped"})

                        elif cmd == "list_models":
                            result = _ollama.list()
                            models = [m.model for m in (result.models or [])]
                            self.cmd_socket.send_json({"models": models})

                        elif cmd == "set_model":
                            model = msg.get("model")
                            if isinstance(model, str) and model.strip():
                                self.ollama_model = model
                            sp.run(["notify-send", "-h", "boolean:transient:true",
                                    "NeuroPipe Assistant", f"Model: {self.ollama_model}"],
                                   capture_output=True)
                            self.cmd_socket.send_json({"model": self.ollama_model})

                        elif cmd == "list_tools":
                            self.cmd_socket.send_json({"tools": self.tool_manager.list_all()})

                        elif cmd == "set_tools":
                            new_config = msg.get("tools", {})
                            if not isinstance(new_config, dict):
                                self.cmd_socket.send_json({"status": "error", "message": "tools must be an object"})
                                continue

                            known_tools = set(self.tool_manager.list_all().keys())
                            validation_error = None
                            for tool_name, level in new_config.items():
                                if tool_name not in known_tools:
                                    validation_error = f"Unknown tool '{tool_name}'"
                                    break
                                if level not in ("allow", "ask", "deny"):
                                    validation_error = f"Invalid permission '{level}' for tool '{tool_name}'"
                                    break
                            if validation_error is None:
                                self.tool_manager.set_config(new_config)
                                self.cmd_socket.send_json({"tools": self.tool_manager.list_all()})
                                continue
                            self.cmd_socket.send_json({"status": "error", "message": validation_error})

                        elif cmd == "grant_tool":
                            tool_name = msg.get("tool", "")
                            self.tool_manager.grant(tool_name)
                            self.cmd_socket.send_json({"status": "granted", "tool": tool_name})

                        elif cmd == "deny_tool":
                            self.cmd_socket.send_json({"status": "denied", "tool": msg.get("tool", "")})

                        elif cmd == "get_state":
                            tts_state = self.send_tts_command({"command": "get_state"})
                            self.cmd_socket.send_json({
                                "mode": self.mode,
                                "busy": self.is_busy(),
                                "model": self.ollama_model,
                                "engine": tts_state.get("engine"),
                                "voice": tts_state.get("voice"),
                                "speed": tts_state.get("speed"),
                                "quality": tts_state.get("quality"),
                            })

                        elif cmd == "get_history":
                            history = self.get_history()
                            self.cmd_socket.send_json({
                                "history": history,
                                "count": len(history),
                            })

                        elif cmd == "reset_memory":
                            result = self.reset_longterm_memory()
                            self.cmd_socket.send_json(result)
                    except Exception as e:
                        print(f"Command error: {e}")
                        self.cmd_socket.send_json(
                            {"status": "error", "message": str(e)}
                        )

                if self.stt_sub in socks:
                    msg = self.stt_sub.recv_json()
                    event = msg.get("event")

                    if event == "transcription":
                        user_text = msg.get("text", "")
                        if self.mode in ("MODE1", "MODE2") and user_text:
                            self.handle_transcription(user_text)

                    elif event == "listening_start":
                        print(".", end="", flush=True)
                        threading.Thread(target=self._warm_tts, daemon=True).start()

        except KeyboardInterrupt:
            print("\nShutting down...")
        finally:
            try:
                self.stop()
            except Exception as e:
                print(f"Shutdown error: {e}")
            try:
                self.memory_store.close()
            except Exception:
                pass


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        description="NeuroPipe Assistant Service"
    )
    parser.add_argument(
        "--model", default=DEFAULT_MODEL,
        help="Ollama model name (default: %(default)s)"
    )
    args = parser.parse_args()

    svc = AssistantService()
    svc.ollama_model = args.model
    svc.run()
