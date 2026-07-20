# Assistant Client

CLI client for the NeuroPipe Assistant Service.

## Usage

### Requirements

- [Ollama](https://ollama.com) — must be installed, running, and have the target model pulled
- The **assistant service** (`neuropipe-assistant.service`) must be running

### Run directly (development)

```bash
# Start session, interruptable only by IPC
uv run python src/assistant_client.py mode1

# Start session with custom model and voice
uv run python src/assistant_client.py mode2 --model llama3.2:3b --engine kokoro --voice af_bella

# Interrupt current response
uv run python src/assistant_client.py interrupt

# Stop session
uv run python src/assistant_client.py stop

# Get service state
uv run python src/assistant_client.py get_state
```

### Run as installed binary

```bash
~/.local/bin/neuro-assistant-client mode2 --model gemma4:cloud
~/.local/bin/neuro-assistant-client interrupt
~/.local/bin/neuro-assistant-client stop
~/.local/bin/neuro-assistant-client get_state
```
