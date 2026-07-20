# Assistant Client

CLI client for the NeuroPipe Assistant Service.

## Usage

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
