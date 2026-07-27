import zmq
import sounddevice as sd
import numpy as np
import collections
import queue as _queue
import threading
import time
from pysilero_vad import SileroVoiceActivityDetector
from engines.parakeet import ParakeetEngine
from neuropipe_config import load_config

# --- CONFIG ---
_CONFIG = load_config()

PUB_ADDR = _CONFIG["ipc"]["stt_pub"]
REP_ADDR = _CONFIG["ipc"]["stt_cmd"]

SAMPLE_RATE = 16000
WINDOW_SIZE = 512
STT_MODEL_NAME = _CONFIG["stt"]["model"]
MODEL_IDLE_TIMEOUT = _CONFIG["stt"]["model_idle_timeout_sec"]  # Unload model after idle

# VAD
VAD_THRESHOLD = _CONFIG["stt"]["vad_threshold"]
DIGITAL_GAIN = _CONFIG["stt"]["digital_gain"]
SILENCE_DURATION_MS = 1000
PRE_RECORD_MS = 500

# Buffer Calcs
CHUNKS_PER_SEC = SAMPLE_RATE // WINDOW_SIZE
MAX_SILENCE_CHUNKS = int(SILENCE_DURATION_MS / 1000 * CHUNKS_PER_SEC)
PRE_RECORD_CHUNKS = int(PRE_RECORD_MS / 1000 * CHUNKS_PER_SEC)
MAX_RECORDING_SECONDS = 15
MAX_RECORDING_CHUNKS = int(MAX_RECORDING_SECONDS * CHUNKS_PER_SEC)


class STTService:
    def __init__(self):
        print(f"Initializing Service for {STT_MODEL_NAME}...")
        self.engine = ParakeetEngine(STT_MODEL_NAME)
        self.vad = SileroVoiceActivityDetector()

        # ZMQ Setup
        self.ctx = zmq.Context()
        self.pub = self.ctx.socket(zmq.PUB)
        self.pub.bind(PUB_ADDR)

        self.rep = self.ctx.socket(zmq.REP)
        self.rep.bind(REP_ADDR)

        # Logic State
        self.mode = _CONFIG["stt"].get("mode", "IDLE")
        self.running = True
        self.stream = None

        # Async Transcription
        self.transcription_queue = _queue.Queue()
        self.result_queue = _queue.Queue()
        self.engine_lock = threading.Lock()

        # Timeout State
        self.last_activity = time.time()

    def float32_to_int16_bytes(self, audio_float):
        return (audio_float * 32767).astype(np.int16).tobytes()

    def start_stream(self):
        """Opens the microphone stream"""
        if self.stream is None:
            print("Microphone: ON")
            self.stream = sd.InputStream(samplerate=SAMPLE_RATE,
                                         blocksize=WINDOW_SIZE,
                                         channels=1, dtype="float32")
            self.stream.start()

    def stop_stream(self):
        """Closes the microphone stream to save resources"""
        if self.stream is not None:
            print("Microphone: OFF")
            self.stream.stop()
            self.stream.close()
            self.stream = None

    def _transcription_worker(self):
        while self.running:
            try:
                audio = self.transcription_queue.get(timeout=0.1)
                self.last_activity = time.time()
                with self.engine_lock:
                    text = self.engine.transcribe(audio)
                self.last_activity = time.time()
                self.result_queue.put(text)
            except _queue.Empty:
                continue
            except Exception as e:
                print(f"Transcription Worker Error: {e}")

    def check_idle_timeout(self):
        """Checks if we should unload the heavy model (non-blocking)."""
        if not self.engine_lock.acquire(blocking=False):
            return  # worker is transcribing, skip
        try:
            if self.engine.is_loaded():
                if time.time() - self.last_activity > MODEL_IDLE_TIMEOUT:
                    print(f"Model idle for {MODEL_IDLE_TIMEOUT}s.")
                    self.engine.unload()
        finally:
            self.engine_lock.release()

    def run(self):
        print("NeuroPipe Service Started")
        print(f"Mode: {self.mode}")

        threading.Thread(target=self._transcription_worker, daemon=True).start()

        # Audio Buffers
        pre_speech_buffer = collections.deque(maxlen=PRE_RECORD_CHUNKS)
        recorded_audio = []
        is_recording = False
        silence_counter = 0

        while self.running:
            # CHECK COMMANDS (Non-Blocking)
            try:
                msg = self.rep.recv_json(flags=zmq.NOBLOCK)
                cmd = msg.get("command")
                print(f"Cmd: {cmd}")

                if cmd == "get_state":
                    self.rep.send_json({
                        "mode": self.mode,
                        "vad_threshold": VAD_THRESHOLD,
                        "sample_rate": SAMPLE_RATE,
                        "model": STT_MODEL_NAME,
                    })

                elif cmd == "set_mode":
                    new_mode = msg.get("mode", "IDLE")

                    # Logic: Handle Stream state based on Mode
                    if new_mode == "IDLE":
                        self.stop_stream()
                    elif new_mode in ["VAD", "MANUAL"]:
                        if self.mode == "IDLE":
                            self.start_stream()

                    self.mode = new_mode

                    # Reset states
                    is_recording = False
                    recorded_audio = []

                    # Reset VAD internal state when switching modes
                    if self.mode == "VAD":
                        self.vad.reset()

                    self.pub.send_json(
                        {"event": "mode_changed", "mode": self.mode})
                    self.rep.send_json({"status": "ok"})

                    # Reset timer on interaction
                    self.last_activity = time.time()

                elif cmd == "manual_stop":
                    if recorded_audio:
                        full_audio = np.concatenate(recorded_audio)
                        self.transcription_queue.put(full_audio)
                        self.last_activity = time.time()

                    self.mode = "IDLE"
                    self.stop_stream()
                    is_recording = False
                    recorded_audio = []
                    self.rep.send_json({"status": "ok"})

            except zmq.Again:
                pass

            # CHECK TRANSCRIPTION RESULTS
            try:
                text = self.result_queue.get_nowait()
                if text.strip():
                    print(f"> {text}")
                    self.pub.send_json({"event": "transcription",
                                        "text": text.strip()})
            except _queue.Empty:
                pass

            # AUDIO PROCESSING
            if self.mode == "IDLE":
                # Check for cleanup while sleeping
                self.check_idle_timeout()
                time.sleep(0.05)
                continue

            try:
                data, _ = self.stream.read(WINDOW_SIZE)
                chunk = data.flatten()

                # Gain & Clip
                if DIGITAL_GAIN != 1.0:
                    chunk = np.clip(chunk * DIGITAL_GAIN, -1.0, 1.0)

                # --- VAD MODE ---
                if self.mode == "VAD":
                    prob = self.vad(self.float32_to_int16_bytes(chunk))

                    if not is_recording:
                        pre_speech_buffer.append(chunk)
                        if prob > VAD_THRESHOLD:
                            is_recording = True
                            print("VAD Start")
                            self.pub.send_json({"event": "listening_start"})
                            recorded_audio.extend(pre_speech_buffer)
                            pre_speech_buffer.clear()
                    else:
                        recorded_audio.append(chunk)
                        if prob < VAD_THRESHOLD:
                            silence_counter += 1
                        else:
                            silence_counter = 0

                        # Stop Conditions
                        if silence_counter > MAX_SILENCE_CHUNKS or len(
                                recorded_audio) > MAX_RECORDING_CHUNKS:
                            print("Processing...")
                            full_audio = np.concatenate(recorded_audio)
                            self.transcription_queue.put(full_audio)
                            self.last_activity = time.time()

                            is_recording = False
                            recorded_audio = []
                            silence_counter = 0
                            self.vad.reset()  # Reset VAD between sentences
                            self.pub.send_json({"event": "listening_end"})

                # --- MANUAL MODE ---
                elif self.mode == "MANUAL":
                    recorded_audio.append(chunk)

                # Check for cleanup even while running (e.g., long listening silence)
                self.check_idle_timeout()

            except Exception as e:
                print(f"Audio Error: {e}")
                # Emergency fallback: if stream crashes, go idle
                self.mode = "IDLE"
                self.stop_stream()


if __name__ == "__main__":
    service = STTService()
    try:
        service.run()
    except KeyboardInterrupt:
        print("Stopping")
        service.stop_stream()
        if service.engine.is_loaded():
            service.engine.unload()
