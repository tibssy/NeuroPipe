# TTS Service

Local Text-to-Speech daemon for NeuroPipe. It receives speak/stop commands via ZeroMQ, generates speech with a selectable engine, and streams playback through the system audio device.

## What this service does

- Exposes command socket: `ipc:///tmp/neuropipe_tts_cmd.sock`
- Exposes event socket: `ipc:///tmp/neuropipe_tts_events.sock`
- Supports commands: `speak`, `stop`, `get_state`
- Supports **two engines**: Kokoro ONNX (default) and Pocket TTS ONNX
- Publishes playback events (`speaking`, `sentence_done`, `interrupted`)
- Auto-unloads engine after idle timeout to free memory
- Models auto-download on first use — no manual download needed

## Prerequisites

- Python `>=3.13, <3.14`
- `uv` installed
- Working audio output (PipeWire/PulseAudio/ALSA)

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

The service validates engine/voice/speed/quality combinations before applying defaults or speaking.
Invalid engine/voice mismatch returns an error and does not update runtime state or config.

### Kokoro (default)

```json
{"command":"speak","text":"Hello","engine":"kokoro","voice":"af_bella","speed":1.0,"quality":"low"}
```

### Pocket TTS

```json
{"command":"speak","text":"Hello","engine":"pocket-tts","voice":"alba","speed":1.0,"quality":"low"}
```

### Stop

```json
{"command":"stop"}
```

### Get state

```json
{"command":"get_state"}
```

## Engines

| Engine | Voices | Quality | Speed |
|---|---|---|---|
| `kokoro` | Built-in voice names (e.g. `af_bella`, `af_heart`) | `low` (fp16), `high` (fp32) | Supported |
| `pocket-tts` | Predefined (`alba`, `azelma`, `cosette`, `eponine`, `fantine`, `javert`, `jean`, `marius`) or custom `.safetensors` path | `low` (int8), `high` (fp32) | Supported |

Models auto-download from Hugging Face / GitHub on first load. Cache location:

- **Kokoro**: `~/.local/share/neuropipe/models/kokoro/`
- **Pocket TTS**: `~/.local/share/neuropipe/models/pocket-tts/`

## Licensing & Attribution

Voice embeddings for pocket-tts are from [Kyutai](https://kyutai.org/), licensed under [CC-BY-4.0](https://creativecommons.org/licenses/by/4.0/).
See [VOICE_CREDITS.md](VOICE_CREDITS.md) for full attribution and prohibited-use terms.
