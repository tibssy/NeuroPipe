import os
import pysbd
from kokoro_onnx import Kokoro
from .base import TTSEngine

# --- CONFIG ---
# Standardizing paths for the ecosystem
BASE_DIR = os.path.expanduser("~/.local/share/neuropipe/models/kokoro")
MODEL_PATH = os.path.join(BASE_DIR, "kokoro-v1.0.onnx")
VOICES_PATH = os.path.join(BASE_DIR, "voices-v1.0.bin")


class KokoroEngine(TTSEngine):
    def __init__(self):
        self.kokoro = None
        self.segmenter = pysbd.Segmenter(language="en", clean=True)
        self.lang = "en-us"

    def load(self):
        if not self.kokoro:
            print(f"[Kokoro] Loading model from {BASE_DIR}...")
            if not os.path.exists(MODEL_PATH):
                raise FileNotFoundError(f"Model not found at {MODEL_PATH}")

            self.kokoro = Kokoro(MODEL_PATH, VOICES_PATH)
            print("[Kokoro] Loaded.")

    def unload(self):
        if self.kokoro:
            print("[Kokoro] Unloading...")
            self.kokoro = None

    def generate(self, text: str, voice: str, speed: float):
        """
        Generator that yields (audio, sample_rate, sentence_text)
        """
        if not self.kokoro:
            self.load()

        # Segment Text
        sentences = self.segmenter.segment(text)

        # Process each sentence
        for sentence in sentences:
            sentence = sentence.strip()
            if not sentence:
                continue

            # Generate Audio
            audio, sr = self.kokoro.create(
                sentence,
                voice=voice,
                speed=speed,
                lang=self.lang
            )

            yield (audio, sr, sentence)