from abc import ABC, abstractmethod
import numpy as np
from typing import Generator

class TTSEngine(ABC):
    @abstractmethod
    def load(self):
        """Load models into memory"""
        pass

    @abstractmethod
    def unload(self):
        """Free memory"""
        pass

    @abstractmethod
    def generate(self, text: str, voice: str, speed: float) -> Generator[tuple[np.ndarray, int, str], None, None]:
        """
        Yields tuples of: (audio_chunk, sample_rate, sentence_text)
        """
        pass