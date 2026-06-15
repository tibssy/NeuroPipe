# NeuroPipe

**NeuroPipe** is a modular, privacy-first AI ecosystem designed for Linux. It acts as the backend infrastructure to connect local Speech-to-Text (STT), Text-to-Speech (TTS), and Large Language Models (LLM) using efficient Linux primitives like **ZeroMQ**, **Unix Sockets**, and **Systemd**.

> **Architecture:**  
> Microphone → [Silero VAD] → [Parakeet TDT Model] → **ZeroMQ Pub/Sub** → Clients (Shell, Assistant, Scripts)

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

- build from source (`TTS only`, `STT only`, or `both`)
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

## Hyprland Integration
Add these bindings to `~/.config/hypr/hyprland.conf`:

```ini
# Neuro STT
bind = SUPER, L, exec, bash -lc 'text=$($HOME/.local/bin/neuro-stt-trigger); [ -n "$text" ] && wtype -d 5 "$text"'

# Neuro TTS
bind = CTRL, R, exec, bash -lc '$HOME/.local/bin/neuro-tts-trigger speak "$(wl-paste)"'
bind = CTRL SHIFT, R, exec, ~/.local/bin/neuro-tts-trigger stop
```

## Niri Integration

Add this style of binding to your Niri config:

```kdl
// Neuro STT
Mod+L { spawn "bash" "-c" "text=$($HOME/.local/bin/neuro-stt-trigger); wtype -d 5 \"$text\""; }

// Neuro TTS
Ctrl+R { spawn "bash" "-c" "$HOME/.local/bin/neuro-tts-trigger speak \"$(wl-paste)\""; }
Ctrl+Shift+R { spawn "~/.local/bin/neuro-tts-trigger" "stop"; }
```
