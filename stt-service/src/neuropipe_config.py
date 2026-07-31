import copy
import os
import tomllib


CONFIG_PATH = os.path.expanduser("~/.config/neuropipe/config.toml")


DEFAULT_CONFIG = {
    "version": 1,
    "ipc": {
        "stt_cmd": "ipc:///tmp/neuropipe_cmd.sock",
        "stt_pub": "ipc:///tmp/neuropipe_pub.sock",
        "tts_cmd": "ipc:///tmp/neuropipe_tts_cmd.sock",
        "tts_events": "ipc:///tmp/neuropipe_tts_events.sock",
        "assistant_cmd": "ipc:///tmp/neuropipe_assistant_cmd.sock",
    },
    "stt": {
        "mode": "IDLE",
        "model": "nemo-parakeet-tdt-0.6b-v3",
        "vad_threshold": 0.5,
        "digital_gain": 3.0,
        "model_idle_timeout_sec": 60,
    },
    "assistant": {
        "default_model": "llama3.2:1b",
        "history_idle_timeout_sec": 3600,
        "memory": {
            "enabled_local": True,
            "enabled_cloud": False,
            "summarize_on_idle": True,
            "summarize_on_stop": True,
            "max_summary_chars": 1200,
            "retrieve_top_k": 4,
            "qdrant_path": "~/.local/share/neuropipe/memory/qdrant",
            "collection": "assistant_memory",
            "embedding_model": "all-minilm",
        },
        "instructions": {
            "system_prompt": "You are a helpful AI voice assistant.\nKeep answers short and conversational.\nThis is a voice-to-voice conversation: assume the user replies by speaking, not typing.\nIf you need confirmation (for example before using a tool in ask mode), request a spoken yes/no response and never ask the user to type.\n/set nothink",
            "tool_usage_policy": "When the user asks about something a tool can help with, call the appropriate tool automatically. If a tool is in ask mode, request spoken permission (yes/no) and continue based on the user's voice response. Do not ask the user to type permission commands.",
        },
        "tools": {
            "open_url": "ask",
            "screenshot": "ask",
            "web_search": "ask",
        },
    },
}


def _deep_merge(base: dict, incoming: dict) -> dict:
    out = copy.deepcopy(base)
    for key, value in incoming.items():
        if isinstance(value, dict) and isinstance(out.get(key), dict):
            out[key] = _deep_merge(out[key], value)
        else:
            out[key] = value
    return out


def _validate(config: dict):
    ipc = config.get("ipc", {})
    for key in ("stt_cmd", "stt_pub"):
        value = ipc.get(key)
        if not isinstance(value, str) or not value.startswith("ipc://"):
            raise ValueError(f"Invalid ipc path for '{key}': {value}")

    stt = config.get("stt", {})
    threshold = stt.get("vad_threshold")
    if not isinstance(threshold, (int, float)) or threshold < 0.0 or threshold > 1.0:
        raise ValueError(f"Invalid stt.vad_threshold: {threshold}")

    gain = stt.get("digital_gain")
    if not isinstance(gain, (int, float)) or gain <= 0:
        raise ValueError(f"Invalid stt.digital_gain: {gain}")

    idle_timeout = stt.get("model_idle_timeout_sec")
    if not isinstance(idle_timeout, int) or idle_timeout < 1:
        raise ValueError(f"Invalid stt.model_idle_timeout_sec: {idle_timeout}")


def load_config() -> dict:
    merged = copy.deepcopy(DEFAULT_CONFIG)
    if os.path.exists(CONFIG_PATH):
        try:
            with open(CONFIG_PATH, "rb") as f:
                parsed = tomllib.load(f)
            if isinstance(parsed, dict):
                merged = _deep_merge(merged, parsed)
        except Exception as e:
            print(f"[config] Failed to read {CONFIG_PATH}: {e}")

    try:
        _validate(merged)
    except ValueError as e:
        print(f"[config] Invalid config at {CONFIG_PATH}: {e}")
        merged = copy.deepcopy(DEFAULT_CONFIG)

    return merged
