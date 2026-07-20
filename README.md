# NeuroPipe

**NeuroPipe** is a modular, privacy-first AI ecosystem designed for Linux. It acts as the backend infrastructure to connect local Speech-to-Text (STT), Text-to-Speech (TTS), and Large Language Models (LLM) using efficient Linux primitives like **ZeroMQ**, **Unix Sockets**, and **Systemd**.

> **Architecture:**  
> Microphone → [Silero VAD] → [Parakeet TDT Model] → **ZeroMQ Pub/Sub** → Clients (Shell, Assistant, Scripts)
> Voice Loop: STT → **Ollama** → TTS (via Assistant Service)

## Features
*   **Zero Latency:** Uses Unix Domain Sockets (IPC) for instant communication.
*   **Local Only:** No data leaves your machine. Powered by ONNX Runtime.
*   **Modular:** The STT service runs as a system daemon. Clients connect only when needed.
*   **Wayland Ready:** Optimized for integration with Hyprland and Sway.

## Installation

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

## Service checks

```bash
systemctl --user status neuropipe-stt.service
systemctl --user status neuropipe-tts.service
systemctl --user status neuropipe-assistant.service
```

## Quick usage

### STT one-shot trigger

```bash
text=$(~/.local/bin/neuro-stt-trigger)
printf 'Heard: %s\n' "$text"
```

### TTS trigger

```bash
~/.local/bin/neuro-tts-trigger speak "Hello from NeuroPipe"
~/.local/bin/neuro-tts-trigger stop
```

### TTS engine selection

Two engines available: `kokoro` (default) and `pocket-tts`. Models auto-download on first use.

```bash
~/.local/bin/neuro-tts-trigger speak "Hello" --engine pocket-tts --voice alba
~/.local/bin/neuro-tts-trigger speak "Hello" --engine kokoro --voice af_bella --speed 1.15 --quality high
```

- `--engine`: `kokoro` or `pocket-tts`
- `--voice`: engine-specific voice name
- `--speed`: playback speed (0.5–2.0)
- `--quality`: `low` (faster) or `high` (better quality)

### Assistant (STT + Ollama + TTS)

Requires [Ollama](https://ollama.com) installed and running with a model pulled.

Two modes:
- **MODE1** — interruptable only by IPC `interrupt` command
- **MODE2** — interruptable by IPC `interrupt` or new voice input

```bash
# Start voice assistant session (MODE2 with voice interrupt)
~/.local/bin/neuro-assistant-client mode2 --model gemma4:cloud

# Start with custom TTS voice
~/.local/bin/neuro-assistant-client mode1 --model llama3.2:3b --engine kokoro --voice af_bella

# Interrupt current response (stays in session)
~/.local/bin/neuro-assistant-client interrupt

# Stop session entirely
~/.local/bin/neuro-assistant-client stop

# Check service state
~/.local/bin/neuro-assistant-client get_state
```

> Note: The `--model` flag is required. If omitted the service defaults to `gemma4:cloud`.

## Hyprland Integration
Add these bindings to `~/.config/hypr/hyprland.conf`:

```ini
# Neuro STT
bind = SUPER, L, exec, bash -lc 'text=$($HOME/.local/bin/neuro-stt-trigger); [ -n "$text" ] && wtype -d 5 "$text"'

# Neuro TTS
bind = CTRL, R, exec, bash -lc '$HOME/.local/bin/neuro-tts-trigger speak "$(wl-paste)"'
bind = CTRL SHIFT, R, exec, ~/.local/bin/neuro-tts-trigger stop

# Neuro Assistant
bind = SUPER, Period, exec, $HOME/.local/bin/neuro-assistant-client mode2 --model gemma4:cloud
bind = SUPER, comma, exec, $HOME/.local/bin/neuro-assistant-client interrupt
```

## Niri Integration

Add this style of binding to your Niri config:

```kdl
// Neuro STT
Mod+L { spawn "bash" "-c" "text=$($HOME/.local/bin/neuro-stt-trigger); wtype -d 5 \"$text\""; }

// Neuro TTS
Ctrl+R { spawn "bash" "-c" "$HOME/.local/bin/neuro-tts-trigger speak \"$(wl-paste)\""; }
Ctrl+Shift+R { spawn "~/.local/bin/neuro-tts-trigger" "stop"; }

// Neuro Assistant
Mod+. { spawn "~/.local/bin/neuro-assistant-client" "mode2" "--model" "gemma4:cloud"; }
Mod+, { spawn "~/.local/bin/neuro-assistant-client" "interrupt"; }
```
