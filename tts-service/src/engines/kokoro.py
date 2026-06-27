import os
import multiprocessing
import queue
import urllib.request
import warnings

from .base import TTSEngine


# --- CONFIG ---
BASE_DIR = os.path.expanduser("~/.local/share/neuropipe/models/kokoro")
MODEL_BASE_URL = "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0"
VOICES_URL = f"{MODEL_BASE_URL}/voices-v1.0.bin"

MODEL_FILES = {
    "low": "kokoro-v1.0.fp16.onnx",
    "high": "kokoro-v1.0.onnx",
}


def _ensure_file(filename, url):
    path = os.path.join(BASE_DIR, filename)
    if os.path.exists(path):
        return path
    print(f"[Kokoro] Downloading {filename}...")
    os.makedirs(BASE_DIR, exist_ok=True)
    urllib.request.urlretrieve(url, path)
    return path


def _worker_process(input_queue, output_queue, quality="low"):
    """Runs in a completely separate process."""
    try:
        import pysbd
        from kokoro_onnx import Kokoro

        model_file = MODEL_FILES[quality]
        model_path = _ensure_file(model_file, f"{MODEL_BASE_URL}/{model_file}")
        voices_path = _ensure_file("voices-v1.0.bin", VOICES_URL)

        print(f"[Kokoro-Worker] Loading Model ({quality})...")
        kokoro = Kokoro(model_path, voices_path)
        segmenter = pysbd.Segmenter(language="en", clean=True)
        lang = "en-us"
        print("[Kokoro-Worker] Ready.")

        output_queue.put(("READY", None))

        while True:
            try:
                task = input_queue.get(timeout=1)
            except queue.Empty:
                continue

            if task is None:
                break

            text, voice, speed = task

            sentences = segmenter.segment(text)

            for sentence in sentences:
                sentence = sentence.strip()
                if not sentence:
                    continue

                audio, sr = kokoro.create(sentence, voice=voice, speed=speed, lang=lang)

                output_queue.put(("AUDIO", (audio, sr, sentence)))

            output_queue.put(("DONE", None))

    except Exception as e:
        import traceback
        output_queue.put(("ERROR", f"{e}\n{traceback.format_exc()}"))


class KokoroEngine(TTSEngine):
    def __init__(self, quality="low"):
        if quality not in ("low", "high"):
            raise ValueError(f"quality must be 'low' or 'high', got '{quality}'")
        self.quality = quality
        self.process = None
        self.input_queue = None
        self.output_queue = None

    def set_quality(self, quality):
        if quality == self.quality:
            return
        if quality not in ("low", "high"):
            raise ValueError(f"quality must be 'low' or 'high', got '{quality}'")
        self.unload()
        self.quality = quality

    def load(self):
        if self.process and self.process.is_alive():
            return

        self.input_queue = multiprocessing.Queue()
        self.output_queue = multiprocessing.Queue()

        self.process = multiprocessing.Process(
            target=_worker_process,
            args=(self.input_queue, self.output_queue, self.quality),
        )
        self.process.start()

        try:
            msg_type, payload = self.output_queue.get(timeout=30)
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
            except Exception:
                pass
            if self.process.is_alive():
                self.process.terminate()
                self.process.join()
            self.process = None
            self.input_queue = None
            self.output_queue = None
            print("[Kokoro] Worker Dead. RAM Reclaimed.")

    def generate(self, text, voice, speed):
        if not self.process or not self.process.is_alive():
            self.load()

        self.input_queue.put((text, voice, speed))

        while True:
            try:
                msg_type, payload = self.output_queue.get(timeout=30)
                if msg_type == "AUDIO":
                    audio, sr, sentence = payload
                    yield (audio, sr, sentence)
                elif msg_type == "DONE":
                    break
                elif msg_type == "ERROR":
                    print(f"[Kokoro] Worker Error: {payload}")
                    break
            except queue.Empty:
                print("[Kokoro] Worker timed out generating audio.")
                break
