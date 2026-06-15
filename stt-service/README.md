# STT Service

Local Speech-to-Text daemon for NeuroPipe. It captures microphone audio, runs VAD and transcription, then publishes events over ZeroMQ IPC sockets.

## What this service does

- Exposes command socket: `ipc:///tmp/neuropipe_cmd.sock`
- Exposes event socket: `ipc:///tmp/neuropipe_pub.sock`
- Supports `IDLE`, `VAD`, and `MANUAL` capture modes
- Uses Silero VAD + Parakeet (`nemo-parakeet-tdt-0.6b-v3`) for transcription
- Auto-unloads model after idle timeout to reclaim RAM

## Prerequisites

- Python `>=3.13, <3.14`
- `uv` installed
- Working audio stack (PipeWire/PulseAudio/ALSA)

## Install dependencies

```bash
uv sync
```

## Run from source

```bash
uv run python src/stt_service.py
```

## Build standalone binary

This repo includes `build.sh`, which compiles a single-file binary with Nuitka and bundles required model data.

```bash
chmod +x build.sh
./build.sh
```

Build output:

- `dist/neuro-stt-service`

## Optional systemd user service

```bash
install -Dm755 dist/neuro-stt-service "$HOME/.local/bin/neuro-stt-service"
install -Dm644 src/service/neuropipe-stt.service "$HOME/.config/systemd/user/neuropipe-stt.service"
systemctl --user daemon-reload
systemctl --user enable --now neuropipe-stt.service
```

## Command API

- `{"command": "set_mode", "mode": "IDLE|VAD|MANUAL"}`
- `{"command": "manual_stop"}`

## Published events

- `mode_changed`
- `listening_start`
- `listening_end`
- `transcription`
