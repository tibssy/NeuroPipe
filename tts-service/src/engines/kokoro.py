import os
import multiprocessing
import queue
from .base import TTSEngine

# --- CONFIG ---
BASE_DIR = os.path.expanduser("~/.local/share/neuropipe/models/kokoro")
MODEL_PATH = os.path.join(BASE_DIR, "kokoro-v1.0.onnx")
VOICES_PATH = os.path.join(BASE_DIR, "voices-v1.0.bin")


def _worker_process(input_queue, output_queue):
    """
    This function runs in a completely separate process.
    It loads the model, processes requests, and dies when told.
    """
    try:
        import pysbd
        from kokoro_onnx import Kokoro

        # Load Model
        print("[Kokoro-Worker] Loading Model...")
        if not os.path.exists(MODEL_PATH):
            output_queue.put(("ERROR", f"Model not found at {MODEL_PATH}"))
            return

        kokoro = Kokoro(MODEL_PATH, VOICES_PATH)
        segmenter = pysbd.Segmenter(language="en", clean=True)
        lang = "en-us"
        print("[Kokoro-Worker] Ready.")

        # Signal ready
        output_queue.put(("READY", None))

        while True:
            # Wait for command: (text, voice, speed) or None to exit
            try:
                task = input_queue.get(timeout=1)  # Check queue often
            except queue.Empty:
                continue

            if task is None:
                break

            text, voice, speed = task

            # Segment
            sentences = segmenter.segment(text)

            for sentence in sentences:
                sentence = sentence.strip()
                if not sentence: continue

                # Generate
                audio, sr = kokoro.create(sentence, voice=voice, speed=speed,
                                          lang=lang)

                # Send back to parent
                output_queue.put(("AUDIO", (audio, sr, sentence)))

            # Signal done with this text block
            output_queue.put(("DONE", None))

    except Exception as e:
        output_queue.put(("ERROR", str(e)))


class KokoroEngine(TTSEngine):
    def __init__(self):
        self.process = None
        self.input_queue = None
        self.output_queue = None

    def load(self):
        if self.process and self.process.is_alive():
            return

        # Start the worker
        self.input_queue = multiprocessing.Queue()
        self.output_queue = multiprocessing.Queue()

        self.process = multiprocessing.Process(
            target=_worker_process,
            args=(self.input_queue, self.output_queue)
        )
        self.process.start()

        # Wait for READY signal
        try:
            msg_type, payload = self.output_queue.get(timeout=10)
            if msg_type == "ERROR":
                raise RuntimeError(payload)
        except queue.Empty:
            self.unload()
            raise RuntimeError("Kokoro Worker failed to start (Timeout)")

    def unload(self):
        if self.process:
            print("[Kokoro] Killing Worker Process...")

            try:
                self.input_queue.put(None)
                self.process.join(timeout=1)
            except:
                pass

            # Force kill if still alive
            if self.process.is_alive():
                self.process.terminate()
                self.process.join()  # Clean up zombie process

            self.process = None
            self.input_queue = None
            self.output_queue = None
            print("[Kokoro] Worker Dead. RAM Reclaimed.")

    def generate(self, text: str, voice: str, speed: float):
        if not self.process or not self.process.is_alive():
            self.load()

        # Send Task
        self.input_queue.put((text, voice, speed))

        # Read Results
        while True:
            # Wait for audio chunks
            try:
                # 5s timeout prevents hanging if worker crashes
                msg_type, payload = self.output_queue.get(timeout=5)

                if msg_type == "AUDIO":
                    audio, sr, sentence = payload
                    yield (audio, sr, sentence)
                elif msg_type == "DONE":
                    break
                elif msg_type == "ERROR":
                    print(f"Worker Error: {payload}")
                    break
            except queue.Empty:
                print("Worker timed out generating audio.")
                break