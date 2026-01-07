import zmq
import sys
import json

# --- CONFIG ---
PUB_ADDR = "ipc:///tmp/neuropipe_pub.sock"
CMD_ADDR = "ipc:///tmp/neuropipe_cmd.sock"


def main():
    ctx = zmq.Context()
    sub = ctx.socket(zmq.SUB)
    sub.connect(PUB_ADDR)
    sub.setsockopt_string(zmq.SUBSCRIBE, "")
    cmd = ctx.socket(zmq.REQ)
    cmd.connect(CMD_ADDR)

    try:
        print("Listening...", file=sys.stderr)
        cmd.send_json({"command": "set_mode", "mode": "VAD"})
        cmd.recv_json()

        while True:
            msg = sub.recv_json()
            event = msg.get("event")

            if event == "transcription":
                text = msg.get("text", "")
                print(text)
                break

            elif event == "listening_start":
                print("Voice Detected...", file=sys.stderr)

    except KeyboardInterrupt:
        print("\nCancelled.", file=sys.stderr)
        sys.exit(1)

    finally:
        cmd.send_json({"command": "set_mode", "mode": "IDLE"})
        cmd.recv_json()


if __name__ == "__main__":
    main()