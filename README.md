# NeuroPipe

**NeuroPipe** is a modular, privacy-first AI ecosystem designed for Linux. It acts as the backend infrastructure to connect local Speech-to-Text (STT), Text-to-Speech (TTS), and Large Language Models (LLM) using efficient Linux primitives like **ZeroMQ**, **Unix Sockets**, and **Systemd**.

> **Architecture:**  
> Microphone 🎤 → [Silero VAD] → [Parakeet TDT Model] → **ZeroMQ Pub/Sub** → Clients (Shell, Assistant, Scripts)

## ✨ Features
*   **Zero Latency:** Uses Unix Domain Sockets (IPC) for instant communication.
*   **Local Only:** No data leaves your machine. Powered by ONNX Runtime.
*   **Modular:** The STT service runs as a system daemon. Clients connect only when needed.
*   **Wayland Ready:** Optimized for integration with Hyprland and Sway.

## System Dependencies

Before installing the Python environment, ensure your system has the necessary build tools and audio libraries.

### Arch Linux (Pacman)
```bash
# Audio & Build Tools
sudo pacman -S base-devel python git pipewire pipewire-pulse alsa-utils

# Wayland Utilities (For Keybinding Trigger)
sudo pacman -S wtype wl-clipboard

# Compilation Tools (If building binaries with Nuitka)
sudo pacman -S patchelf ccache zstandard
```

### Debian / Ubuntu
```bash
sudo apt install build-essential python3-dev git pipewire pipewire-pulse alsa-utils
sudo apt install wtype wl-clipboard patchelf ccache
```

## Installation

### 1. Clone the Repository
```bash
git clone https://github.com/tibssy/NeuroPipe.git
cd NeuroPipe
```

### 2. Setup STT Service
The service handles the microphone and AI inference.
```bash
cd stt-service
python -m venv venv
source venv/bin/activate
pip install -r requirements.txt
```

### 3. Setup Client & Trigger
The client sends commands to the service (Start, Stop, VAD Mode).
```bash
cd ../stt-client
python -m venv venv
source venv/bin/activate
pip install -r requirements.txt
```

## Hyprland Integration (Voice Typing)
You can compile the client trigger into a standalone binary for instant startup latency using Nuitka.

### 1. Build the Service:
```bash
cd ../stt-service
source venv/bin/activate
pip install nuitka zstandard
python -m nuitka --onefile \
        --include-package-data=pysilero_vad \
        --include-data-files=$VIRTUAL_ENV/lib/python*/site-packages/pysilero_vad/*.bin=pysilero_vad/ \
        --include-package-data=onnx_asr \
        --output-dir=dist \
        --output-filename=neuro-stt-service \
        --assume-yes-for-downloads \
        --lto=yes \
        --python-flag=no_site \
        src/stt_service.py
mv dist/neuro-stt-service ~/.local/bin/neuro-stt-service
cp src/service/neuropipe-stt.service ~/.config/systemd/user/neuropipe-stt.service
```

### 2. Start and verify the Service:
```bash
systemctl --user daemon-reload
systemctl --user enable --now neuropipe-stt.service
systemctl --user status neuropipe-stt.service
```


### 3. Build the Trigger:
```bash
cd ../stt-client
source venv/bin/activate
pip install nuitka zstandard
python -m nuitka --onefile --output-dir=dist --output-filename=neuro-trigger src/trigger_input.py
mv dist/trigger_input ~/.local/bin/neuro-trigger
```

### 4. Configure Hyprland:
Add this to your ~/.config/hypr/hyprland.conf. This binds Super + L to listen to your voice and type the result into the active window.
```Ini
bind = Super, L, exec, bash -c 'text=$($HOME/.local/bin/neuro-trigger); wtype -d 5 "$text"'
```
