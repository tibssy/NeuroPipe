import zmq
import time
import sounddevice as sd
import numpy as np
import threading
import queue
import gc

# Engine Imports
from engines.kokoro import KokoroEngine

# from engines.piper import PiperEngine

# --- CONFIG ---
CMD_ADDR = "ipc:///tmp/neuropipe_tts_cmd.sock"
PUB_ADDR = "ipc:///tmp/neuropipe_tts_events.sock"
IDLE_TIMEOUT = 60


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
            # "piper": PiperEngine()
        }
        self.active_engine = None
        self.active_engine_name = None

        # State
        self.audio_queue = queue.Queue()
        self.interrupt_event = threading.Event()
        self.current_sentence = ""  # Tracks what is currently being spoken

        # Start Player
        threading.Thread(target=self._player_loop, daemon=True).start()

    def _switch_engine(self, name):
        if name == self.active_engine_name: return
        if self.active_engine:
            self.active_engine.unload()
            gc.collect()
        self.active_engine = self.engines[name]
        self.active_engine.load()
        self.active_engine_name = name

    def _player_loop(self):
        while True:
            try:
                # Queue item: (audio_array, sample_rate, sentence_text)
                chunk, sr, sentence = self.audio_queue.get(timeout=1)

                if self.interrupt_event.is_set():
                    self.audio_queue.task_done()
                    continue

                # Update State & Notify
                self.current_sentence = sentence
                self.pub_socket.send_json(
                    {"event": "speaking", "sentence": sentence})

                # Play (Blocking)
                sd.play(chunk, samplerate=sr, blocking=True)

                # Check interruption on playback
                if self.interrupt_event.is_set():
                    sd.stop()
                    # Notify interruption with the specific sentence that got cut
                    self.pub_socket.send_json({
                        "event": "interrupted",
                        "last_sentence": sentence
                    })
                else:
                    self.pub_socket.send_json(
                        {"event": "sentence_done", "sentence": sentence})

                self.audio_queue.task_done()

            except queue.Empty:
                # Handle Idle Unload logic...
                pass

    def run(self):
        print("TTS Service Running...")
        while True:
            msg = self.cmd_socket.recv_json()
            cmd = msg.get("command")

            if cmd == "speak":
                self.interrupt_event.clear()
                text = msg.get("text")
                engine = msg.get("engine", "kokoro")

                voice = msg.get("voice", "af_bella")
                speed = msg.get("speed", 1.0)

                self._switch_engine(engine)
                self.cmd_socket.send_json({"status": "queued"})

                # Generate and Queue
                for audio, sr, sent in self.active_engine.generate(text, voice, speed):
                    if self.interrupt_event.is_set(): break
                    self.audio_queue.put((audio, sr, sent))

            elif cmd == "stop":
                print("Interrupt Signal!")
                self.interrupt_event.set()
                sd.stop()

                # Clear pending queue
                with self.audio_queue.mutex:
                    self.audio_queue.queue.clear()

                self.cmd_socket.send_json({"status": "stopped",
                                           "last_sentence": self.current_sentence})

            elif cmd == "get_state":
                self.cmd_socket.send_json(
                    {"active_engine": self.active_engine_name})


if __name__ == "__main__":
    TTSService().run()