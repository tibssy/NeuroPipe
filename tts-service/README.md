# TTS Service

Local Text-to-Speech daemon for NeuroPipe. It receives speak/stop commands via ZeroMQ, generates speech with a selectable engine, and streams playback through the system audio device.

## What this service does

- Exposes command socket: `ipc:///tmp/neuropipe_tts_cmd.sock`
- Exposes event socket: `ipc:///tmp/neuropipe_tts_events.sock`
- Supports commands: `speak`, `stop`, `get_state`
- Uses Kokoro ONNX engine by default
- Publishes playback events (`speaking`, `sentence_done`, `interrupted`)
- Auto-unloads engine after idle timeout to free memory

## Prerequisites

- Python `>=3.13, <3.14`
- `uv` installed
- Working audio output (PipeWire/PulseAudio/ALSA)
- Kokoro model files expected at:
  - `~/.local/share/neuropipe/models/kokoro/kokoro-v1.0.onnx`
  - `~/.local/share/neuropipe/models/kokoro/voices-v1.0.bin`

## Install dependencies

```bash
uv sync
```

## Run from source

```bash
uv run python src/tts_service.py
```

## Build standalone binary

`build.sh` compiles a one-file executable with Nuitka and includes runtime data files needed by Kokoro and espeak.

```bash
chmod +x build.sh
./build.sh
```

Build output:

- `dist/neuro-tts-service`

## Optional systemd user service

```bash
install -Dm755 dist/neuro-tts-service "$HOME/.local/bin/neuro-tts-service"
install -Dm644 src/service/neuropipe-tts.service "$HOME/.config/systemd/user/neuropipe-tts.service"
systemctl --user daemon-reload
systemctl --user enable --now neuropipe-tts.service
```

## Command payload examples

```json
{"command":"speak","text":"Hello from NeuroPipe","engine":"kokoro","voice":"af_bella","speed":1.0}
```

```json
{"command":"stop"}
```

```json
{"command":"get_state"}
```
