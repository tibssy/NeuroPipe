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

### Speak text (Kokoro, default)

```bash
uv run python src/tts_client.py speak "Hello from NeuroPipe"
```

### Speak text (Pocket TTS)

```bash
uv run python src/tts_client.py speak "Hello" --engine pocket-tts --voice alba
```

### Custom voice, speed, quality

```bash
uv run python src/tts_client.py speak "Fast sample" --voice af_bella --speed 1.15 --engine kokoro --quality high
```

### Pocket TTS voices

Available: `alba`, `azelma`, `cosette`, `eponine`, `fantine`, `javert`, `jean`, `marius`

```bash
uv run python src/tts_client.py speak "Bonjour" --engine pocket-tts --voice cosette --speed 1.0 --quality low
```

### Stop playback

```bash
uv run python src/tts_client.py stop
```

### Monitor events only

```bash
uv run python src/tts_client.py monitor
```

## CLI options

| Flag | Description | Default |
|---|---|---|
| `--engine` | TTS engine (`kokoro` or `pocket-tts`) | `kokoro` |
| `--voice` | Voice name | `af_bella` (kokoro) / `alba` (pocket-tts) |
| `--speed` | Playback speed (0.5–2.0) | `1.0` |
| `--quality` | Audio quality (`low` or `high`) | `low` |

## Build standalone binary

```bash
chmod +x build.sh
./build.sh
```

Build output:

- `dist/neuro-tts-trigger`

You can copy this binary to `~/.local/bin` and invoke it directly in scripts, keybindings, or automation.
