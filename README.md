# NeuroPipe

**NeuroPipe** is a modular, privacy-first AI ecosystem designed for Linux. It acts as the backend infrastructure to connect local Speech-to-Text (STT), Text-to-Speech (TTS), and Large Language Models (LLM) using efficient Linux primitives like **ZeroMQ**, **Unix Sockets**, and **Systemd**.

> **Architecture:**  
> Microphone → [Silero VAD] → [Parakeet TDT Model] → **ZeroMQ Pub/Sub** → Clients (Shell, Assistant, Scripts)
> Voice Loop: STT → **Ollama** → TTS (via Assistant Service)

## Features
*   **Zero Latency:** Uses Unix Domain Sockets (IPC) for instant communication.
*   **Local Only:** No data leaves your machine. Powered by ONNX Runtime.
*   **Modular:** The STT service runs as a system daemon. Clients connect only when needed.
*   **Wayland Ready:** Tested on Niri, GNOME, KDE Plasma, and Hyprland with UWSM.

## Installation

### Dependencies

Runtime dependencies checked by the installer:

- `systemctl`
- `wtype`
- `wl-copy` (from `wl-clipboard`)
- `pw-cli` (from `pipewire`)

Build dependencies checked for source installs:

- `gcc`
- `g++`
- `make`
- `cargo` (Rust toolchain for all services and `neuro-ipc`)

Assistant dependency:

- [Ollama](https://ollama.com) installed and running, with your target model pulled
- `mpv` + [mpv-mpris](https://github.com/hoyon/mpv-mpris) + `yt-dlp` — used by the `open_url` tool to play video/audio links (YouTube, Vimeo, direct media files) via MPRIS so `media_control` can control them

### 1. Clone the repository
```bash
git clone https://github.com/tibssy/NeuroPipe.git
cd NeuroPipe
```

### 2. Run the installer

Use the root installer script. It provides interactive menus for:

- build from source (`TTS`, `STT`, `Assistant`, or `All services`)
- use prebuilt Linux binaries (`x86_64` or `arm64`, auto-detected)
- safe confirmation before copying files and enabling services

```bash
./install.sh
```

### Optional: one-liner install method

If you prefer not to keep a local clone, you can run the installer directly from the latest `main` tarball:

```bash
tmp_dir="$(mktemp -d)" && curl -fsSL https://github.com/tibssy/NeuroPipe/archive/refs/heads/main.tar.gz | tar -xz -C "$tmp_dir" && bash "$tmp_dir/NeuroPipe-main/install.sh"
```

The installer will:

- build or download selected binaries
- copy binaries to `~/.local/bin`
- install service units to `~/.config/systemd/user`
- run `systemctl --user daemon-reload`
- enable and start selected services

It also checks required dependencies and prints package-manager-specific install hints if something is missing.

### GUI tool startup notes

Some assistant tools (`screenshot`, `open_url`) require a fully initialized graphical session environment
(`WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`, `DBUS_SESSION_BUS_ADDRESS`).

Known-good setups:

- Niri
- GNOME
- KDE Plasma
- Hyprland with UWSM

For plain Hyprland and MangoWC, you may need compositor-specific startup wiring to import your session
environment into the systemd user manager before NeuroPipe services start.

Recommended pattern:

```bash
dbus-update-activation-environment --systemd --all
```

If GUI-dependent tools fail after boot but work after restarting `neuropipe-assistant.service`, this is usually
an environment propagation issue in compositor startup ordering.

## Service checks

```bash
systemctl --user status neuropipe-stt.service
systemctl --user status neuropipe-tts.service
systemctl --user status neuropipe-assistant.service
```

## Configuration

NeuroPipe uses a unified config file:

- `~/.config/neuropipe/config.toml`
- Use `config.example.toml` in this repository as a starting template

Services load this file on startup and keep settings in memory for low-latency runtime behavior.
Config changes from CLI commands are persisted atomically to disk. Manual file edits are applied after service restart.

Commands that persist config:

- `neuro-ipc tts set-state ...`
- `neuro-ipc assistant set-model ...`
- `neuro-ipc assistant set-tools ...`

Config helpers:

- `neuro-ipc config show`
- `neuro-ipc config validate`
- `neuro-ipc config path`

Optional favorites in config can constrain cycling:

- `tts.favorites` is used by `neuro-ipc tts set-state --voice next|prev`
- `assistant.favorites.models` is used by `neuro-ipc assistant set-model next|prev`
- Empty favorites lists keep the old behavior (cycle all available voices/models)

Media ducking (assistant):

- `assistant.duck_media` (`true`/`false`) — lower the media player volume while the
  user is speaking or while the assistant's TTS is talking, so voice input can be
  heard over a playing video or song.
- `assistant.duck_volume` (0.0–1.0, default `0.1`) — the volume ducked media drops to.
  Requires `playerctl`. Volume is restored when the utterance/TTS finishes or the session stops.

The installer seeds `~/.config/neuropipe/config.toml` from `config.example.toml` if the config file does not already exist.

## Quick usage

### STT one-shot trigger

```bash
text=$(~/.local/bin/neuro-ipc stt trigger)
printf 'Heard: %s\n' "$text"
```

### TTS trigger

```bash
~/.local/bin/neuro-ipc tts speak "Hello from NeuroPipe"
~/.local/bin/neuro-ipc tts stop
```

### TTS engine selection

Two engines available: `kokoro` (default) and `pocket-tts`. Models auto-download on first use.

```bash
~/.local/bin/neuro-ipc tts speak "Hello" --engine pocket-tts --voice alba
~/.local/bin/neuro-ipc tts speak "Hello" --engine kokoro --voice af_bella --speed 1.15 --quality high
```

- `--engine`: `kokoro` or `pocket-tts`
- `--voice`: engine-specific voice name
- `--speed`: playback speed (0.5–2.0)
- `--quality`: `low` (faster) or `high` (better quality)

Pocket-tts voice embeddings are from [Kyutai](https://kyutai.org/), licensed under [CC-BY-4.0](tts-service-rs/VOICE_CREDITS.md).

STT models are from [NVIDIA Parakeet](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) (CC-BY-4.0), [Silero VAD](https://github.com/snakers4/silero-vad) (MIT), and [Pipecat Smart Turn](https://github.com/pipecat-ai/smart-turn) (BSD-2-Clause); see [stt-service-rs/MODEL_CREDITS.md](stt-service-rs/MODEL_CREDITS.md).

### Assistant (STT + Ollama + TTS)

Requires [Ollama](https://ollama.com) installed and running with a model pulled.

Two modes:
- **MODE1** — interruptable only by IPC `interrupt` command
- **MODE2** — interruptable by IPC `interrupt` or new voice input

```bash
# Start voice assistant session (MODE2 with voice interrupt)
~/.local/bin/neuro-ipc assistant mode2 --model gemma4:cloud

# Start with custom TTS voice
~/.local/bin/neuro-ipc assistant mode1 --model llama3.2:3b --engine kokoro --voice af_bella

# Interrupt current response (stays in session)
~/.local/bin/neuro-ipc assistant interrupt

# Stop session entirely
~/.local/bin/neuro-ipc assistant stop

# Check service state
~/.local/bin/neuro-ipc assistant get-state
```

> Note: The `--model` flag is required. If omitted the service defaults to `gemma4:cloud`.

## Hyprland Integration
Add these bindings to `~/.config/hypr/hyprland.conf`:

```ini
# Neuro STT
bind = SUPER, L, exec, bash -lc 'text=$($HOME/.local/bin/neuro-ipc stt trigger); [ -n "$text" ] && wtype -d 5 "$text"'

# Neuro TTS
bind = CTRL, R, exec, bash -lc '$HOME/.local/bin/neuro-ipc tts speak "$(wl-paste)"'
bind = CTRL SHIFT, R, exec, ~/.local/bin/neuro-ipc tts stop

# Neuro Assistant
bind = SUPER, Period, exec, $HOME/.local/bin/neuro-ipc assistant mode2 --model gemma4:cloud
bind = SUPER, comma, exec, $HOME/.local/bin/neuro-ipc assistant interrupt
```

## Niri Integration

Add this style of binding to your Niri config:

```kdl
// Neuro STT
Mod+L { spawn "bash" "-c" "text=$($HOME/.local/bin/neuro-ipc stt trigger); wtype -d 5 \"$text\""; }

// Neuro TTS
Ctrl+R { spawn "bash" "-c" "$HOME/.local/bin/neuro-ipc tts speak \"$(wl-paste)\""; }
Ctrl+Shift+R {   spawn "~/.local/bin/neuro-ipc" "tts" "stop"; }

// Neuro Assistant
Mod+. {   spawn "~/.local/bin/neuro-ipc" "assistant" "mode2" "--model" "gemma4:cloud"; }
Mod+, {   spawn "~/.local/bin/neuro-ipc" "assistant" "interrupt"; }
```
