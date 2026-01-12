import multiprocessing
import queue
import numpy as np


def _worker_process(input_queue, output_queue, model_name):
    """
    Runs in a separate process.
    """
    try:
        # Import inside process to avoid pollution
        import onnx_asr


        print(f"[Parakeet-Worker] Loading {model_name}...")
        model = onnx_asr.load_model(model_name, quantization="int8")
        print("[Parakeet-Worker] Ready.")

        output_queue.put(("READY", None))

        while True:
            try:
                task = input_queue.get(timeout=1)
            except queue.Empty:
                continue

            if task is None:
                break

            audio_array = task
            text = model.recognize(audio_array)
            output_queue.put(("RESULT", text))

    except Exception as e:
        output_queue.put(("ERROR", str(e)))


class ParakeetEngine:
    def __init__(self, model_name):
        self.model_name = model_name
        self.process = None
        self.input_queue = None
        self.output_queue = None

    def is_loaded(self):
        return self.process is not None and self.process.is_alive()

    def load(self):
        if self.is_loaded(): return

        # Use 'spawn' to avoid deadlocks with Silero's ONNX Runtime in parent
        ctx = multiprocessing.get_context('spawn')

        self.input_queue = ctx.Queue()
        self.output_queue = ctx.Queue()

        self.process = ctx.Process(
            target=_worker_process,
            args=(self.input_queue, self.output_queue, self.model_name)
        )
        self.process.start()

        try:
            msg_type, payload = self.output_queue.get(
                timeout=20)  # 20s for slow disk load
            if msg_type == "ERROR":
                raise RuntimeError(payload)
        except queue.Empty:
            self.unload()
            raise RuntimeError("Parakeet Worker failed to start (Timeout)")

    def unload(self):
        if self.process:
            print("[Parakeet] Unloading Worker...")
            try:
                self.input_queue.put(None)
                self.process.join(timeout=1)
            except:
                pass

            if self.process.is_alive():
                self.process.terminate()

            self.process = None
            self.input_queue = None
            self.output_queue = None
            print("[Parakeet] RAM Reclaimed.")

    def transcribe(self, audio_array: np.ndarray) -> str:
        if not self.is_loaded():
            self.load()

        # Clear any old garbage in the queue from previous runs
        while not self.output_queue.empty():
            try:
                self.output_queue.get_nowait()
            except:
                break

        self.input_queue.put(audio_array)

        try:
            msg_type, payload = self.output_queue.get(timeout=10)
            if msg_type == "RESULT":
                return payload
            elif msg_type == "ERROR":
                print(f"Worker Error: {payload}")
                return ""
        except queue.Empty:
            print("Transcription Timed Out")
            return ""
        return ""