import zmq
import argparse


PUB_ADDR = "ipc:///tmp/neuropipe_pub.sock"
REP_ADDR = "ipc:///tmp/neuropipe_cmd.sock"

def send_command(cmd_dict):
    ctx = zmq.Context()
    sock = ctx.socket(zmq.REQ)
    sock.connect(REP_ADDR)
    sock.send_json(cmd_dict)
    # Blocking wait for "ok" to ensure service handled it
    sock.recv_json()
    sock.close()

def listen_loop():
    ctx = zmq.Context()
    sock = ctx.socket(zmq.SUB)
    sock.connect(PUB_ADDR)
    sock.setsockopt_string(zmq.SUBSCRIBE, "")
    print("NeuroPipe Listener Connected...")
    try:
        while True:
            msg = sock.recv_json()
            if msg.get("event") == "transcription":
                print(f"User: {msg['text']}")
            elif msg.get("event") == "listening_start":
                print("...", end="\r", flush=True)
    except KeyboardInterrupt:
        pass

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--vad", action="store_true", help="Set Mode: VAD")
    parser.add_argument("--idle", action="store_true", help="Set Mode: IDLE")
    parser.add_argument("--record-start", action="store_true", help="Push-To-Talk Start")
    parser.add_argument("--record-stop", action="store_true", help="Push-To-Talk Stop")
    parser.add_argument("--listen", action="store_true", help="Listen for events")
    args = parser.parse_args()

    if args.listen: listen_loop()
    elif args.vad: send_command({"command": "set_mode", "mode": "VAD"})
    elif args.idle: send_command({"command": "set_mode", "mode": "IDLE"})
    elif args.record_start: send_command({"command": "set_mode", "mode": "MANUAL"})
    elif args.record_stop: send_command({"command": "manual_stop"})