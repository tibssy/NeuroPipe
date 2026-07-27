import os
import tomllib
from pathlib import Path

CONFIG_DIR = os.path.expanduser("~/.config/neuropipe")
CONFIG_FILE = os.path.join(CONFIG_DIR, "config.toml")

DEFAULTS = {
    "assistant": {
        "model": "llama3.2:1b",
        "history_timeout": 3600,
        "ollama_sock": "/tmp/ollama.sock",
        "system_prompt": "You are a helpful AI voice assistant.\nKeep answers short and conversational.\n/set nothink\n\n{tool_descriptions}",
    },
    "tts": {
        "engine": "kokoro",
        "voice": "af_bella",
        "speed": 1.0,
        "quality": "high",
        "idle_timeout": 60,
    },
    "stt": {
        "model": "nemo-parakeet-tdt-0.6b-v3",
        "sample_rate": 16000,
        "window_size": 512,
        "vad_threshold": 0.5,
        "digital_gain": 3.0,
        "silence_duration_ms": 1000,
        "pre_record_ms": 500,
        "max_recording_sec": 15,
        "model_idle_timeout": 60,
    },
    "tools": {},
}


def load() -> dict:
    cfg = DEFAULTS.copy()
    if not os.path.isfile(CONFIG_FILE):
        return cfg
    try:
        with open(CONFIG_FILE, "rb") as f:
            overrides = tomllib.load(f)
        for section, values in overrides.items():
            if section in cfg and isinstance(values, dict):
                cfg[section].update(values)
            else:
                cfg[section] = values
    except Exception as e:
        print(f"[config] Failed to load {CONFIG_FILE}: {e}")
    return cfg
