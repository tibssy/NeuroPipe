import os
import zmq
import time
import sounddevice as sd
import threading
import queue
import subprocess as sp

# Engine Imports
from engines.kokoro import KokoroEngine
from engines.pocket_tts import PocketTTSEngine
# from engines.piper import PiperEngine
from neuropipe_config import load_config

# --- CONFIG ---
_CONFIG = load_config()

CMD_ADDR = _CONFIG["ipc"]["tts_cmd"]
PUB_ADDR = _CONFIG["ipc"]["tts_events"]


class TTSService:
    def __init__(self):
        self.ctx = zmq.Context()

        # Command Socket (Input)
        self.cmd_socket = self.ctx.socket(zmq.REP)
        self.cmd_socket.bind(CMD_ADDR)

        # Event Socket (Output)
        self.pub_socket = self.ctx.socket(zmq.PUB)
        self.pub_socket.bind(PUB_ADDR)

        # Engines
        self.engines = {
            "kokoro": KokoroEngine(),
            "pocket-tts": PocketTTSEngine(),
            # "piper": PiperEngine()
        }
        self.active_engine = None
        self.active_engine_name = None

        # State
        self.audio_queue = queue.Queue()
        self.interrupt_event = threading.Event()
        self.current_sentence = ""
        self.last_activity = time.time()

        # Defaults (can be changed via set_state)
        self.default_engine = _CONFIG["tts"]["defaults"]["engine"]
        self.default_voice = _CONFIG["tts"]["defaults"]["voice"]
        self.default_speed = _CONFIG["tts"]["defaults"]["speed"]
        self.default_quality = _CONFIG["tts"]["defaults"]["quality"]
        self.idle_timeout = _CONFIG["tts"]["defaults"]["idle_timeout_sec"]

        # Start Player
        threading.Thread(target=self._player_loop, daemon=True).start()

    def _switch_engine(self, name):
        if name == self.active_engine_name: return
        if self.active_engine:
            self.active_engine.unload()
        self.active_engine = self.engines[name]
        self.active_engine.load()
        self.active_engine_name = name

    def _player_loop(self):
        stream = None
        current_sr = None

        while True:
            try:
                # Queue item: (audio_array, sample_rate, sentence_text)
                chunk, sr, sentence = self.audio_queue.get(timeout=1)

                if self.interrupt_event.is_set():
                    if stream:
                        stream.abort(ignore_errors=True)
                    self.audio_queue.task_done()
                    continue

                # (Re)create stream if sample rate changed
                if sr != current_sr:
                    if stream:
                        stream.close()
                    try:
                        stream = sd.OutputStream(samplerate=sr, channels=1,
                                                 dtype='float32')
                        stream.start()
                        current_sr = sr
                    except Exception as e:
                        print(f"Stream create error: {e}")
                        self.audio_queue.task_done()
                        continue

                # Update State & Notify
                self.current_sentence = sentence
                self.pub_socket.send_json(
                    {"event": "speaking", "sentence": sentence})

                # --- PLAYBACK LOGIC ---
                if self.interrupt_event.is_set():
                    stream.abort(ignore_errors=True)
                else:
                    try:
                        stream.write(chunk)
                    except Exception as e:
                        print(f"Playback Error: {e}")

                # Check result
                if self.interrupt_event.is_set():
                    self.pub_socket.send_json({
                        "event": "interrupted",
                        "last_sentence": sentence
                    })
                else:
                    self.pub_socket.send_json(
                        {"event": "sentence_done", "sentence": sentence})

                self.audio_queue.task_done()
                self.current_sentence = ""
                self.last_activity = time.time()

            except queue.Empty:
                # --- IDLE CHECK ---
                if self.active_engine and (
                        time.time() - self.last_activity > self.idle_timeout):
                    print(f"Idle for {self.idle_timeout}s. Releasing resources.")
                    self.active_engine.unload()
                    self.active_engine = None
                    self.active_engine_name = None
                    print("System is now in Cold Standby.")

    def _validate_tts_state(self, engine, voice, speed, quality):
        if engine not in self.engines:
            return f"Invalid engine '{engine}'. Expected one of: {', '.join(sorted(self.engines.keys()))}"

        if quality not in ("low", "high"):
            return f"Invalid quality '{quality}'. Expected 'low' or 'high'."

        if not isinstance(speed, (int, float)) or speed < 0.5 or speed > 2.0:
            return "Invalid speed. Expected a number between 0.5 and 2.0."

        if not isinstance(voice, str) or not voice.strip():
            return "Invalid voice. Voice must be a non-empty string."

        if voice.endswith(".safetensors"):
            if engine != "pocket-tts":
                return "Custom .safetensors voices are only supported by engine 'pocket-tts'."
            expanded = os.path.abspath(os.path.expanduser(voice))
            if os.path.exists(expanded):
                return None
            return "Voice file path must point to an existing .safetensors file."

        engine_obj = self.engines.get(engine)
        if engine_obj and hasattr(engine_obj, "list_voices"):
            voices = engine_obj.list_voices()
            if voice in voices:
                return None

        return f"Voice '{voice}' is not available for engine '{engine}'."

    def _generate_audio(self, text, voice, speed):
        try:
            for audio, sr, sent in self.active_engine.generate(text, voice, speed):
                if self.interrupt_event.is_set():
                    break
                self.audio_queue.put((audio, sr, sent))
        except Exception as e:
            print(f"[TTS] Generation interrupted: {e}")

    def run(self):
        print("TTS Service Running...")

        poller = zmq.Poller()
        poller.register(self.cmd_socket, zmq.POLLIN)

        while True:
            socks = dict(poller.poll(timeout=500))

            if self.cmd_socket in socks:
                msg = self.cmd_socket.recv_json()
                cmd = msg.get("command")

                if cmd == "speak":
                    self.interrupt_event.set()
                    self.last_activity = time.time()
                    text = msg.get("text")
                    engine = msg.get("engine", self.default_engine)

                    voice = msg.get("voice", self.default_voice)
                    speed = msg.get("speed", self.default_speed)
                    quality = msg.get("quality", self.default_quality)

                    validation_err = self._validate_tts_state(engine, voice, speed, quality)
                    if validation_err:
                        self.cmd_socket.send_json({"status": "error", "message": validation_err})
                        self.interrupt_event.clear()
                        continue

                    self._switch_engine(engine)

                    if quality and hasattr(self.active_engine, "set_quality"):
                        self.active_engine.set_quality(quality)

                    self.interrupt_event.clear()
                    self.cmd_socket.send_json({"status": "queued"})

                    threading.Thread(
                        target=self._generate_audio,
                        args=(text, voice, speed),
                        daemon=True,
                    ).start()

                elif cmd == "stop":
                    print("Interrupt Signal!")
                    self.interrupt_event.set()

                    last_sentence = self.current_sentence

                    with self.audio_queue.mutex:
                        self.audio_queue.queue.clear()

                    self.cmd_socket.send_json({"status": "stopped",
                                               "last_sentence": last_sentence})

                elif cmd == "warm":
                    engine = msg.get("engine", self.default_engine)
                    quality = msg.get("quality", self.default_quality)
                    self._switch_engine(engine)
                    if quality and hasattr(self.active_engine, "set_quality"):
                        self.active_engine.set_quality(quality)
                    self.last_activity = time.time()
                    self.cmd_socket.send_json({"status": "ok"})

                elif cmd == "get_state":
                    self.cmd_socket.send_json({
                        "engine": self.default_engine,
                        "voice": self.default_voice,
                        "speed": self.default_speed,
                        "quality": self.default_quality,
                        "speaking": not self.audio_queue.empty() or bool(self.current_sentence),
                    })

                elif cmd == "list_voices":
                    engine_name = msg.get("engine", self.default_engine)
                    engine = self.engines.get(engine_name)
                    if engine and hasattr(engine, "list_voices"):
                        voices = engine.list_voices()
                    else:
                        voices = []
                    self.cmd_socket.send_json({"voices": voices})

                elif cmd == "set_state":
                    next_engine = msg.get("engine", self.default_engine)
                    next_voice = msg.get("voice", self.default_voice)
                    next_speed = msg.get("speed", self.default_speed)
                    next_quality = msg.get("quality", self.default_quality)

                    validation_err = self._validate_tts_state(
                        next_engine,
                        next_voice,
                        next_speed,
                        next_quality,
                    )
                    if validation_err:
                        self.cmd_socket.send_json({"status": "error", "message": validation_err})
                        continue

                    self.default_engine = next_engine
                    self.default_voice = next_voice
                    self.default_speed = next_speed
                    self.default_quality = next_quality

                    voice_label = os.path.splitext(os.path.basename(self.default_voice))[0]
                    sp.run(
                        ["notify-send", "-h", "boolean:transient:true", "NeuroPipe TTS",
                         f"{self.default_engine} | {voice_label} | {self.default_speed}x | {self.default_quality}"],
                        capture_output=True,
                    )
                    self.cmd_socket.send_json({
                        "status": "ok",
                        "engine": self.default_engine,
                        "voice": self.default_voice,
                        "speed": self.default_speed,
                        "quality": self.default_quality,
                    })


if __name__ == "__main__":
    TTSService().run()
