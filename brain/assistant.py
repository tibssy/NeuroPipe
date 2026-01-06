import zmq
import sys
from ollama import chat

# --- CONFIG ---
# ZMQ Settings
SUB_ADDR = "ipc:///tmp/neuropipe_pub.sock"  # listen
CMD_ADDR = "ipc:///tmp/neuropipe_cmd.sock"  # send commands

# Ollama Settings
OLLAMA_MODEL = 'gemma3:27b-cloud'
SYSTEM_MESSAGE = {
    'role': 'system',
    'content': 'You are a helpful AI voice assistant. Keep answers short and conversational.'
}


class Assistant:
    def __init__(self):
        # Setup ZMQ to Listen to STT
        self.ctx = zmq.Context()
        self.sub = self.ctx.socket(zmq.SUB)
        self.sub.connect(SUB_ADDR)
        self.sub.setsockopt_string(zmq.SUBSCRIBE, "")  # Listen to everything

        # Setup ZMQ to Control STT (Start/Stop)
        self.cmd = self.ctx.socket(zmq.REQ)
        self.cmd.connect(CMD_ADDR)

        # Chat History
        self.history = [SYSTEM_MESSAGE]

    def send_stt_command(self, command, mode=None):
        """Helper to tell STT service what to do"""
        msg = {"command": command}
        if mode:
            msg["mode"] = mode

        self.cmd.send_json(msg)
        self.cmd.recv_json()  # Wait for ACK

    def ask_ollama(self, text):
        """Send text to Ollama and stream the result"""
        print(f"\nUser: {text}")
        print("AI: ", end="", flush=True)

        self.history.append({'role': 'user', 'content': text})

        full_response = ""

        # Stream response
        for chunk in chat(model=OLLAMA_MODEL, messages=self.history,
                          stream=True):
            if chunk.message.content:
                content = chunk.message.content
                print(content, end="", flush=True)
                full_response += content

                # --- FUTURE TTS HOOK ---

        print("\n")
        self.history.append({'role': 'assistant', 'content': full_response})

    def run(self):
        print(f"Brain connected to NeuroPipe...")
        print(f"Model: {OLLAMA_MODEL}")

        # Wake up the STT Service automatically
        print("Activating Ears (VAD Mode)...")
        self.send_stt_command("set_mode", "VAD")

        try:
            while True:
                # Wait for data from STT Service
                msg = self.sub.recv_json()

                event = msg.get("event")

                if event == "transcription":
                    user_text = msg.get("text")
                    self.ask_ollama(user_text)

                elif event == "listening_start":
                    # Visual feedback
                    print("...", end="\r", flush=True)

        except KeyboardInterrupt:
            print("\nShutting down...")
        finally:
            # STT back to sleep when Brain dies
            print("Deactivating Ears...")
            self.send_stt_command("set_mode", "IDLE")


if __name__ == "__main__":
    bot = Assistant()
    bot.run()