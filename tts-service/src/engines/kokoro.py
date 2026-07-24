import os
import multiprocessing
from multiprocessing import shared_memory
import queue

import numpy as np

from .base import TTSEngine


# --- CONFIG ---
BASE_DIR = os.path.expanduser("~/.local/share/neuropipe/models/kokoro")
MODEL_FILES = {
    "low": "kokoro-v1.0.fp16.onnx",
    "high": "kokoro-v1.0.onnx",
}


def _ensure_file(filename):
    path = os.path.join(BASE_DIR, filename)
    if not os.path.exists(path):
        raise FileNotFoundError(
            f"Model file '{filename}' not found in {BASE_DIR}. "
            f"Run the install script or manually place the file."
        )
    return path


def _worker_process(input_queue, output_queue, quality="low"):
    """Runs in a completely separate process."""
    try:
        import pysbd
        from kokoro_onnx import Kokoro

        model_file = MODEL_FILES[quality]
        model_path = _ensure_file(model_file)
        voices_path = _ensure_file("voices-v1.0.bin")

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

                if audio.nbytes > 4096:
                    shm = shared_memory.SharedMemory(create=True, size=audio.nbytes)
                    buf = np.ndarray(audio.shape, dtype=audio.dtype, buffer=shm.buf)
                    buf[:] = audio[:]
                    output_queue.put(("AUDIO_SHM", (shm.name, audio.shape, audio.dtype, sr, sentence)))
                else:
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

    def list_voices(self):
        voices_path = os.path.join(BASE_DIR, "voices-v1.0.bin")
        if not os.path.exists(voices_path):
            return []
        voices = np.load(voices_path, allow_pickle=True)
        return sorted(voices.keys())

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
                elif msg_type == "AUDIO_SHM":
                    shm_name, shape, dtype, sr, sentence = payload
                    shm = shared_memory.SharedMemory(name=shm_name)
                    audio = np.ndarray(shape, dtype=dtype, buffer=shm.buf).copy()
                    shm.close()
                    shm.unlink()
                    yield (audio, sr, sentence)
                elif msg_type == "DONE":
                    break
                elif msg_type == "ERROR":
                    print(f"[Kokoro] Worker Error: {payload}")
                    break
            except queue.Empty:
                print("[Kokoro] Worker timed out generating audio.")
                break
