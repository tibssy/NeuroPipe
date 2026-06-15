# STT Client

Command-line tools for controlling and consuming events from the NeuroPipe STT service.

This folder contains two entry points:

- `stt_client.py`: full control client (set mode, listen, manual stop)
- `stt_trigger.py`: one-shot trigger that waits for one transcription and prints it

## Sockets used

- Command socket: `ipc:///tmp/neuropipe_cmd.sock`
- Event socket: `ipc:///tmp/neuropipe_pub.sock`

## Prerequisites

- Python `>=3.13, <3.14`
- `uv` installed
- `stt-service` must be running

## Install dependencies

```bash
uv sync
```

## Run `stt_client.py`

### Enable always-on VAD mode

```bash
uv run python src/stt_client.py --vad
```

### Return service to idle mode

```bash
uv run python src/stt_client.py --idle
```

### Push-to-talk flow

```bash
uv run python src/stt_client.py --record-start
uv run python src/stt_client.py --record-stop
```

### Listen for transcription events

```bash
uv run python src/stt_client.py --listen
```

## Run `stt_trigger.py` (one-shot)

```bash
uv run python src/stt_trigger.py
```

Behavior:

- sets mode to `VAD`
- waits until one `transcription` event arrives
- prints recognized text to stdout
- restores service mode to `IDLE`

This makes it ideal for window-manager keybindings and shell pipelines.

## Build standalone binary

```bash
chmod +x build.sh
./build.sh
```

Build output:

- `dist/neuro-stt-trigger`
