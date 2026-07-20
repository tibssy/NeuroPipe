# Assistant Service

Voice assistant daemon for NeuroPipe. Connects STT → Ollama → TTS into a conversational loop, controllable via ZeroMQ IPC.

## Requirements

- [Ollama](https://ollama.com) — must be installed, running, and have the target model pulled

## What this service does

- Exposes command socket: `ipc:///tmp/neuropipe_assistant_cmd.sock`
- Subscribes to STT events at `ipc:///tmp/neuropipe_pub.sock`
- Sends commands to STT (`set_mode`) and TTS (`speak`, `stop`)
- Supports **two modes**:
  - **MODE1** — interruptable only by IPC `interrupt` command
  - **MODE2** — interruptable by IPC `interrupt` or new voice input
- History persists across sessions and auto-clears after 1h of inactivity
- Ollama model, TTS engine, and TTS voice configurable via client

## Commands

| Command | Payload | Description |
|---|---|---|
| `mode1` | `{model?, engine?, voice?}` | Start session (IPC interrupt only) |
| `mode2` | `{model?, engine?, voice?}` | Start session (IPC + voice interrupt) |
| `interrupt` | — | Stop current AI response, stay in mode |
| `stop` | — | Stop session, set STT to IDLE |
| `get_state` | — | Return mode, busy, model, engine, voice |

## Usage

### Run directly (development)

```bash
# Start a session (use PYTHONUNBUFFERED=1 when running in background)
PYTHONUNBUFFERED=1 uv run python src/assistant_service.py
PYTHONUNBUFFERED=1 uv run python src/assistant_service.py --model llama3.2:3b
```

### Run as installed systemd service

```bash
# After install via ./install.sh
systemctl --user enable --now neuropipe-assistant.service
journalctl --user -u neuropipe-assistant.service -f
```
