import copy
import os
import tomllib

import tomli_w


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

    instructions = assistant.get("instructions", {})
    if not isinstance(instructions, dict):
        raise ValueError("assistant.instructions must be a table")
    for key in ("system_prompt", "tool_usage_policy"):
        value = instructions.get(key)
        if not isinstance(value, str) or not value.strip():
            raise ValueError(f"Invalid assistant.instructions.{key}")

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


def _atomic_write(path: str, data: bytes):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp_path = f"{path}.tmp.{os.getpid()}"

    with open(tmp_path, "wb") as f:
        f.write(data)
        f.flush()
        os.fsync(f.fileno())

    os.replace(tmp_path, path)

    dir_fd = os.open(os.path.dirname(path), os.O_DIRECTORY)
    try:
        os.fsync(dir_fd)
    finally:
        os.close(dir_fd)


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


def save_config(config: dict):
    validate_config(config)
    text = tomli_w.dumps(config)
    _atomic_write(CONFIG_PATH, text.encode("utf-8"))


def update_config(mutator):
    current = load_config()
    mutator(current)
    save_config(current)
