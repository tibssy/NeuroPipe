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
    "tts": {
        "defaults": {
            "engine": "kokoro",
            "voice": "af_bella",
            "speed": 1.0,
            "quality": "high",
            "idle_timeout_sec": 60,
        }
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
            "embedding_model": "embeddinggemma",
        },
        "instructions": {
            "system_prompt": "You are a helpful AI voice assistant.\nKeep answers short and conversational.\n/set nothink",
            "tool_usage_policy": "When the user asks about something a tool can help with, call the appropriate tool automatically. Do not ask for permission.",
        },
        "tools": {},
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


def _validate_ipc(ipc: dict):
    for key in ("stt_cmd", "stt_pub", "tts_cmd", "tts_events", "assistant_cmd"):
        value = ipc.get(key)
        if not isinstance(value, str) or not value.startswith("ipc://"):
            raise ValueError(f"Invalid ipc path for '{key}': {value}")


def _validate_tts_defaults(defaults: dict):
    engine = defaults.get("engine")
    if engine not in ("kokoro", "pocket-tts"):
        raise ValueError(f"Invalid tts.defaults.engine: {engine}")

    quality = defaults.get("quality")
    if quality not in ("low", "high"):
        raise ValueError(f"Invalid tts.defaults.quality: {quality}")

    speed = defaults.get("speed")
    if not isinstance(speed, (int, float)) or speed < 0.5 or speed > 2.0:
        raise ValueError(f"Invalid tts.defaults.speed: {speed}")

    idle_timeout = defaults.get("idle_timeout_sec")
    if not isinstance(idle_timeout, int) or idle_timeout < 1:
        raise ValueError(f"Invalid tts.defaults.idle_timeout_sec: {idle_timeout}")

    voice = defaults.get("voice")
    if not isinstance(voice, str) or not voice.strip():
        raise ValueError("Invalid tts.defaults.voice")


def _validate_assistant(assistant: dict):
    model = assistant.get("default_model")
    if not isinstance(model, str) or not model.strip():
        raise ValueError("Invalid assistant.default_model")

    timeout = assistant.get("history_idle_timeout_sec")
    if not isinstance(timeout, int) or timeout < 1:
        raise ValueError(f"Invalid assistant.history_idle_timeout_sec: {timeout}")

    memory = assistant.get("memory", {})
    if not isinstance(memory, dict):
        raise ValueError("assistant.memory must be a table")

    for key in ("enabled_local", "enabled_cloud", "summarize_on_idle", "summarize_on_stop"):
        value = memory.get(key)
        if not isinstance(value, bool):
            raise ValueError(f"Invalid assistant.memory.{key}: {value}")

    max_summary = memory.get("max_summary_chars")
    if not isinstance(max_summary, int) or max_summary < 200:
        raise ValueError(f"Invalid assistant.memory.max_summary_chars: {max_summary}")

    top_k = memory.get("retrieve_top_k")
    if not isinstance(top_k, int) or top_k < 1 or top_k > 20:
        raise ValueError(f"Invalid assistant.memory.retrieve_top_k: {top_k}")

    for key in ("qdrant_path", "collection", "embedding_model"):
        value = memory.get(key)
        if not isinstance(value, str) or not value.strip():
            raise ValueError(f"Invalid assistant.memory.{key}: {value}")

    tools = assistant.get("tools", {})
    if not isinstance(tools, dict):
        raise ValueError("assistant.tools must be a table")
    for tool_name, level in tools.items():
        if level not in ("allow", "ask", "deny"):
            raise ValueError(f"Invalid permission '{level}' for tool '{tool_name}'")


def validate_config(config: dict):
    _validate_ipc(config.get("ipc", {}))
    _validate_tts_defaults(config.get("tts", {}).get("defaults", {}))
    _validate_assistant(config.get("assistant", {}))


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
        validate_config(merged)
    except ValueError as e:
        print(f"[config] Invalid config at {CONFIG_PATH}: {e}")
        merged = copy.deepcopy(DEFAULT_CONFIG)

    return merged
