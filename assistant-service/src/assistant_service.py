import zmq
import re
import os
import time
import threading
import httpx
import subprocess as sp
import argparse
from ollama import Client

OLLAMA_SOCK = "/tmp/ollama.sock"
if os.path.exists(OLLAMA_SOCK):
    _transport = httpx.HTTPTransport(uds=OLLAMA_SOCK)
    _ollama = Client(transport=_transport)
else:
    _ollama = Client()

def chat(*args, **kwargs):
    return _ollama.chat(*args, **kwargs)

SENTENCE_END = re.compile(r'[.!?](?:\s|$)')

CMD_ADDR = "ipc:///tmp/neuropipe_assistant_cmd.sock"
STT_PUB_ADDR = "ipc:///tmp/neuropipe_pub.sock"
STT_CMD_ADDR = "ipc:///tmp/neuropipe_cmd.sock"
TTS_CMD_ADDR = "ipc:///tmp/neuropipe_tts_cmd.sock"
TTS_EVENTS_ADDR = "ipc:///tmp/neuropipe_tts_events.sock"

DEFAULT_MODEL = "llama3.2:1b"
HISTORY_IDLE_TIMEOUT = 3600

SYSTEM_MESSAGE = {
    'role': 'system',
    'content': (
        'You are a helpful AI voice assistant. '
        'Keep answers short and conversational.\n/set nothink'
    ),
}


class AssistantService:
    def __init__(self):
        self.ctx = zmq.Context()

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
        self.history = [SYSTEM_MESSAGE]
        self.last_activity = time.time()
        self._pending_sentences = 0
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

    def speak(self, text):
        if not text.strip() or self.cancel_event.is_set():
            return None
        cmd = {"command": "speak", "text": text, "speed": 1.0}
        try:
            reply = self.send_tts_command(cmd)
            self._pending_sentences += 1
            return reply
        except zmq.ZMQError as e:
            print(f"speak error: {e}")
            return None

    def _truncate_history(self, last_spoken):
        while self.history and self.history[-1]['role'] == 'user':
            self.history.pop()
        if last_spoken:
            self.history.append({'role': 'assistant', 'content': last_spoken})

    def ask_ollama(self, text):
        print(f"\nYou: {text}")
        print("AI: ", end="", flush=True)

        self.history.append({'role': 'user', 'content': text})

        full_response = ""
        sentence_buffer = ""

        tts_batch_buffer = []
        tts_batch_chars = 0
        MAX_BATCH_SENTENCES = 3
        MAX_BATCH_CHARS = 150
        is_first = True

        try:
            for chunk in chat(
                model=self.ollama_model,
                messages=self.history,
                stream=True,
            ):
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
        except Exception as e:
            print(f"\n[Ollama Error: {e}]")
            self.last_activity = time.time()
            return

        if self.cancel_event.is_set():
            print("\n[Interrupted]\n")
            return

        print("\n")
        self.history.append({'role': 'assistant', 'content': full_response})
        self.last_activity = time.time()

        remaining = sentence_buffer.strip()
        if remaining:
            if self.cancel_event.is_set():
                return
            if self.mode == "MODE2" and tts_batch_buffer:
                tts_batch_buffer.append(remaining)
                self.speak(" ".join(tts_batch_buffer))
            else:
                self.speak(remaining)
        elif self.mode == "MODE2" and tts_batch_buffer:
            self.speak(" ".join(tts_batch_buffer))

    def is_busy(self):
        return self.ollama_thread is not None and self.ollama_thread.is_alive()

    def interrupt(self):
        if not self.is_busy():
            return ""
        self.cancel_event.set()
        reply = self.stop_tts()
        last_sentence = reply.get("last_sentence", "") if reply else ""
        self.ollama_thread.join(timeout=5)
        self._truncate_history(last_sentence)
        self.cancel_event.clear()
        if self.mode == "MODE1":
            self.set_stt_mode("VAD")
        return last_sentence

    def _warm_tts(self):
        try:
            self.send_tts_command({"command": "warm"})
        except zmq.ZMQError:
            pass

    def start_session(self, mode, model=None, engine=None, voice=None):
        if time.time() - self.last_activity > HISTORY_IDLE_TIMEOUT:
            print("Idle > 1h, clearing history.")
            self.history = [SYSTEM_MESSAGE]

        if model:
            self.ollama_model = model

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
        if self.is_busy():
            self.interrupt()
        self.set_stt_mode("IDLE")
        self.mode = "IDLE"
        sp.run(["notify-send", "-h", "boolean:transient:true", "NeuroPipe", "Idle"], capture_output=True)

    def _process_and_respond(self, text):
        self._pending_sentences = 0

        if self.mode == "MODE1":
            self.set_stt_mode("IDLE")
            tts_sock = self.ctx.socket(zmq.SUB)
            tts_sock.connect(TTS_EVENTS_ADDR)
            tts_sock.setsockopt_string(zmq.SUBSCRIBE, "")
        else:
            tts_sock = None

        self.ask_ollama(text)

        if self.mode == "MODE1":
            remaining = self._pending_sentences
            while remaining > 0 and not self.cancel_event.is_set():
                try:
                    msg = tts_sock.recv_json(flags=zmq.NOBLOCK)
                    event = msg.get("event")
                    if event in ("sentence_done", "interrupted"):
                        remaining -= 1
                except zmq.Again:
                    time.sleep(0.05)
            tts_sock.close()
            self.set_stt_mode("VAD")

    def handle_transcription(self, text):
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
