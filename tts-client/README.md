# TTS Client

CLI client for controlling the NeuroPipe TTS service over ZeroMQ IPC.

## What this client does

- Sends commands to `ipc:///tmp/neuropipe_tts_cmd.sock`
- Subscribes to events from `ipc:///tmp/neuropipe_tts_events.sock`
- Supports speaking text, stopping playback, and event monitoring

## Prerequisites

- Python `>=3.13, <3.14`
- `uv` installed
- `tts-service` must be running

## Install dependencies

```bash
uv sync
```

## Run from source

### Speak text

```bash
uv run python src/tts_client.py speak "Testing NeuroPipe speech"
```

### Use custom voice/speed/engine

```bash
uv run python src/tts_client.py speak "Fast sample" --voice af_bella --speed 1.15 --engine kokoro
```

### Stop playback

```bash
uv run python src/tts_client.py stop
```

### Monitor events only

```bash
uv run python src/tts_client.py monitor
```

## Build standalone binary

```bash
chmod +x build.sh
./build.sh
```

Build output:

- `dist/neuro-tts-trigger`

You can copy this binary to `~/.local/bin` and invoke it directly in scripts, keybindings, or automation.
