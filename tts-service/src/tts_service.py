import zmq
import time
import sounddevice as sd
import threading
import queue
import gc

# Engine Imports
from engines.kokoro import KokoroEngine
from engines.pocket_tts import PocketTTSEngine
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
            "pocket-tts": PocketTTSEngine(),
            # "piper": PiperEngine()
        }
        self.active_engine = None
        self.active_engine_name = None

        # State
        self.audio_queue = queue.Queue()
        self.interrupt_event = threading.Event()
        self.current_sentence = ""  # Tracks what is currently being spoken
        self.last_activity = time.time()

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

                # --- PLAYBACK LOGIC ---
                try:
                    with sd.OutputStream(samplerate=sr, channels=1,
                                         dtype='float32') as stream:
                        block_size = 2048
                        total_samples = len(chunk)

                        for i in range(0, total_samples, block_size):
                            if self.interrupt_event.is_set():
                                break

                            data_slice = chunk[i: i + block_size]
                            stream.write(data_slice)

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
                self.last_activity = time.time()

            except queue.Empty:
                # --- IDLE CHECK ---
                if self.active_engine and (
                        time.time() - self.last_activity > IDLE_TIMEOUT):
                    print(f"Idle for {IDLE_TIMEOUT}s. Releasing resources.")
                    self.active_engine.unload()
                    self.active_engine = None
                    self.active_engine_name = None
                    gc.collect()
                    print("System is now in Cold Standby.")

    def run(self):
        print("TTS Service Running...")
        while True:
            msg = self.cmd_socket.recv_json()
            cmd = msg.get("command")

            if cmd == "speak":
                self.last_activity = time.time()
                self.interrupt_event.clear()
                text = msg.get("text")
                engine = msg.get("engine", "kokoro")

                voice = msg.get("voice", "af_bella")
                speed = msg.get("speed", 1.0)
                quality = msg.get("quality")

                self._switch_engine(engine)

                # Apply quality setting if engine supports it
                if quality and hasattr(self.active_engine, "set_quality"):
                    self.active_engine.set_quality(quality)

                self.cmd_socket.send_json({"status": "queued"})

                # Generate and Queue
                for audio, sr, sent in self.active_engine.generate(text, voice, speed):
                    if self.interrupt_event.is_set(): break
                    self.audio_queue.put((audio, sr, sent))

            elif cmd == "stop":
                print("Interrupt Signal!")
                self.interrupt_event.set()

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