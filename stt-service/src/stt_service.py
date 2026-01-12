import zmq
import sounddevice as sd
import numpy as np
import collections
import time
from pysilero_vad import SileroVoiceActivityDetector
from engines.parakeet import ParakeetEngine

# --- CONFIG ---
PUB_ADDR = "ipc:///tmp/neuropipe_pub.sock"
REP_ADDR = "ipc:///tmp/neuropipe_cmd.sock"

SAMPLE_RATE = 16000
WINDOW_SIZE = 512
STT_MODEL_NAME = "nemo-parakeet-tdt-0.6b-v3"
MODEL_IDLE_TIMEOUT = 60  # Unload model after 60s of no transcription

# VAD
VAD_THRESHOLD = 0.5
DIGITAL_GAIN = 3.0
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
        self.mode = "IDLE"
        self.running = True
        self.stream = None

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

    def check_idle_timeout(self):
        """Checks if we should unload the heavy model"""
        if self.engine.is_loaded():
            if time.time() - self.last_activity > MODEL_IDLE_TIMEOUT:
                print(f"Model idle for {MODEL_IDLE_TIMEOUT}s.")
                self.engine.unload()

    def run(self):
        print("NeuroPipe Service Started")
        print(f"Mode: {self.mode}")

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

                if cmd == "set_mode":
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
                    # Force Transcribe logic
                    if recorded_audio:
                        full_audio = np.concatenate(recorded_audio)
                        text = self.engine.transcribe(full_audio)
                        if text.strip():
                            self.pub.send_json({"event": "transcription",
                                                "text": text.strip()})

                        self.last_activity = time.time()

                    # Go to IDLE and Close Stream
                    self.mode = "IDLE"
                    self.stop_stream()
                    is_recording = False
                    recorded_audio = []
                    self.rep.send_json({"status": "ok"})

            except zmq.Again:
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
                            text = self.engine.transcribe(full_audio)

                            if text.strip():
                                print(f"> {text}")
                                self.pub.send_json({"event": "transcription",
                                                    "text": text.strip()})

                            # Update activity
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