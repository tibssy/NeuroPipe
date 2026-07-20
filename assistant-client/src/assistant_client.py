import zmq
import json
import argparse

CMD_ADDR = "ipc:///tmp/neuropipe_assistant_cmd.sock"
TIMEOUT_MS = 10000


def send_command(cmd_dict):
    ctx = zmq.Context()
    sock = ctx.socket(zmq.REQ)
    sock.setsockopt(zmq.RCVTIMEO, TIMEOUT_MS)
    sock.connect(CMD_ADDR)
    sock.send_json(cmd_dict)
    reply = sock.recv_json()
    sock.close()
    ctx.term()
    return reply


def main():
    parser = argparse.ArgumentParser(
        description="NeuroPipe Assistant Client"
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    p_mode1 = subparsers.add_parser(
        "mode1",
        help="Start session (interruptable only by IPC)"
    )
    p_mode1.add_argument("--model", help="Ollama model name")
    p_mode1.add_argument("--engine", help="TTS engine (kokoro, pocket-tts)")
    p_mode1.add_argument("--voice", help="TTS voice name or path")

    p_mode2 = subparsers.add_parser(
        "mode2",
        help="Start session (interruptable by IPC and voice)"
    )
    p_mode2.add_argument("--model", help="Ollama model name")
    p_mode2.add_argument("--engine", help="TTS engine (kokoro, pocket-tts)")
    p_mode2.add_argument("--voice", help="TTS voice name or path")

    subparsers.add_parser(
        "interrupt", help="Interrupt the current AI response"
    )
    subparsers.add_parser("stop", help="Stop session and go idle")
    subparsers.add_parser("get_state", help="Get current service state")

    args = parser.parse_args()

    try:
        if args.command in ("mode1", "mode2"):
            state = send_command({"command": "get_state"})
            if state.get("busy"):
                send_command({"command": "interrupt"})

            cmd = {"command": args.command}
            if args.model:
                cmd["model"] = args.model
            if args.engine:
                cmd["engine"] = args.engine
            if args.voice:
                cmd["voice"] = args.voice
            reply = send_command(cmd)
        else:
            reply = send_command({"command": args.command})

        print(json.dumps(reply, indent=2))
    except zmq.ZMQError as e:
        print(f"Error: {e}")
        print(f"Is the assistant service running on {CMD_ADDR}?")
        raise SystemExit(1)


if __name__ == "__main__":
    main()
