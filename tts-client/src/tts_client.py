import zmq
import argparse
import sys
import threading

# --- CONFIG ---
CMD_ADDR = "ipc:///tmp/neuropipe_tts_cmd.sock"
PUB_ADDR = "ipc:///tmp/neuropipe_tts_events.sock"


def listen_to_events():
    """Background thread to print what the bot is doing"""
    ctx = zmq.Context()
    sub = ctx.socket(zmq.SUB)
    sub.connect(PUB_ADDR)
    sub.setsockopt_string(zmq.SUBSCRIBE, "")

    print(f"Listening for events on {PUB_ADDR}...")
    try:
        while True:
            msg = sub.recv_json()
            event = msg.get("event")

            if event == "speaking":
                print(f"Speaking: '{msg.get('sentence')}'")
            elif event == "sentence_done":
                # print(f"Finished: '{msg.get('sentence')}'")
                pass
            elif event == "interrupted":
                print(f"INTERRUPTED at: '{msg.get('last_sentence')}'")
    except Exception as e:
        pass


def send_command(cmd_dict):
    """Sends JSON command and prints reply"""
    ctx = zmq.Context()
    req = ctx.socket(zmq.REQ)
    req.connect(CMD_ADDR)

    print(f"Sending: {cmd_dict}")
    req.send_json(cmd_dict)

    reply = req.recv_json()
    print(f"Reply: {reply}")
    req.close()


def main():
    parser = argparse.ArgumentParser(description="NeuroPipe TTS Client")
    subparsers = parser.add_subparsers(dest="action", required=True)

    # Speak Command
    p_speak = subparsers.add_parser("speak", help="Send text to TTS")
    p_speak.add_argument("text", type=str, help="Text to speak")
    p_speak.add_argument("--voice", type=str, default="af_heart",
                         help="Voice ID")
    p_speak.add_argument("--speed", type=float, default=1.0, help="Speed")
    p_speak.add_argument("--quality", type=str, default=None,
                         choices=["low", "high"],
                         help="Quality: low=faster, high=better audio")
    p_speak.add_argument("--engine", type=str, default="kokoro",
                         help="Engine (kokoro/piper)")

    # Stop Command
    p_stop = subparsers.add_parser("stop",
                                   help="Interrupt playback immediately")

    # Monitor Command
    p_mon = subparsers.add_parser("monitor", help="Just listen to events")

    args = parser.parse_args()

    if args.action == "monitor":
        listen_to_events()
        return

    # start a listener thread to see the output immediately
    t = threading.Thread(target=listen_to_events, daemon=True)
    t.start()

    if args.action == "speak":
        cmd = {
            "command": "speak",
            "text": args.text,
            "voice": args.voice,
            "speed": args.speed,
            "engine": args.engine,
        }
        if args.quality:
            cmd["quality"] = args.quality
        send_command(cmd)
        # Keep alive briefly to receive events
        try:
            input("\nPress Enter to exit (or wait for speech to finish)...\n")
        except:
            pass

    elif args.action == "stop":
        send_command({"command": "stop"})


if __name__ == "__main__":
    main()