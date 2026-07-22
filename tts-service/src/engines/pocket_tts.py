import json
import math
import multiprocessing
import os
import queue

import numpy as np
import onnxruntime as ort
from huggingface_hub import snapshot_download
from safetensors import safe_open

from .base import TTSEngine


HF_REPO = "KevinAHM/pocket-tts-onnx"
BUNDLE = "english_2026-04"
BASE_DIR = os.path.expanduser("~/.local/share/neuropipe/models/pocket-tts")
BUNDLE_DIR = os.path.join(BASE_DIR, "onnx", BUNDLE)

DTYPE_MAP = {"float32": np.float32, "float16": np.float16, "int64": np.int64, "bool": np.bool_}


def _filled(shape, dtype, fill):
    if fill == "nan":
        return np.full(shape, np.nan, dtype=dtype)
    return np.ones(shape, dtype=dtype) if fill == "ones" else np.zeros(shape, dtype=dtype)


def _init_state(manifest):
    return {e["input_name"]: _filled(e["shape"], DTYPE_MAP[e["dtype"]], e["fill"]) for e in manifest}


def _update(state, result, manifest, offset):
    for e in manifest:
        state[e["input_name"]] = result[offset + e["index"]]


MAX_TOKEN_PER_CHUNK = 50


def _find_boundary_indices(tokens, boundary_tokens):
    indices = [0]
    prev_boundary = False
    for idx, token in enumerate(tokens):
        if token in boundary_tokens:
            prev_boundary = True
        else:
            if prev_boundary:
                indices.append(idx)
            prev_boundary = False
    indices.append(len(tokens))
    return indices


def _segments_from_boundaries(tokens, indices, tokenizer):
    segments = []
    for i in range(len(indices) - 1):
        text = tokenizer.Decode(tokens[indices[i]:indices[i + 1]])
        segments.append((indices[i + 1] - indices[i], text))
    return segments


def _split_into_best_sentences(text, tokenizer):
    prepared = text.strip()
    tokens = tokenizer.Encode(prepared)

    eos_tokens = set(tokenizer.Encode(".!...?")[1:])
    boundaries = _find_boundary_indices(tokens, eos_tokens)
    segments = _segments_from_boundaries(tokens, boundaries, tokenizer)

    fallback_tokens = set(tokenizer.Encode(",;:")[1:])
    refined = []
    for count, seg_text in segments:
        if count <= MAX_TOKEN_PER_CHUNK:
            refined.append((count, seg_text))
            continue
        sub_tokens = tokenizer.Encode(seg_text.strip())
        sub_bounds = _find_boundary_indices(sub_tokens, fallback_tokens)
        sub_segs = _segments_from_boundaries(sub_tokens, sub_bounds, tokenizer)
        if len(sub_segs) > 1:
            refined.extend(sub_segs)
        else:
            refined.append((count, seg_text))

    chunks = []
    cur_text = ""
    cur_count = 0
    for count, seg_text in refined:
        if not cur_text:
            cur_text = seg_text
            cur_count = count
            continue
        if cur_count + count > MAX_TOKEN_PER_CHUNK:
            chunks.append(cur_text.strip())
            cur_text = seg_text
            cur_count = count
        else:
            cur_text += " " + seg_text
            cur_count += count
    if cur_text:
        chunks.append(cur_text.strip())

    return chunks


def _change_speed(audio, speed):
    if speed == 1.0:
        return audio
    new_len = int(len(audio) / speed)
    indices = np.linspace(0, len(audio) - 1, new_len)
    return np.interp(indices, np.arange(len(audio)), audio).astype(np.float32)


def _load_voice_state(path, manifest):
    with safe_open(path, framework="np") as f:
        raw = {}
        for key in f.keys():
            mod, k = key.split("/", 1)
            raw.setdefault(mod, {})[k] = f.get_tensor(key)

    state = {}
    for e in manifest:
        ms = raw.get(e["module"])
        if ms is None:
            state[e["input_name"]] = _filled(e["shape"], DTYPE_MAP[e["dtype"]], e["fill"])
            continue
        t = ms.get(e["key"])
        if t is None and e["key"] == "step":
            off = ms.get("offset")
            t = np.asarray(off, dtype=np.int64).reshape(1) if off is not None else None
        if t is None:
            state[e["input_name"]] = _filled(e["shape"], DTYPE_MAP[e["dtype"]], e["fill"])
            continue
        t = np.asarray(t, dtype=DTYPE_MAP[e["dtype"]])
        target = tuple(e["shape"])
        if t.shape == target:
            state[e["input_name"]] = t.copy()
        else:
            a = _filled(target, DTYPE_MAP[e["dtype"]], e["fill"])
            slc = tuple(slice(0, min(s, d)) for s, d in zip(t.shape, target))
            a[slc] = t[slc]
            state[e["input_name"]] = a
    return state


def _ensure_bundle():
    """Download the ONNX bundle if not cached locally."""
    if not os.path.exists(os.path.join(BUNDLE_DIR, "bundle.json")):
        print(f"[PocketTTS] Downloading bundle {BUNDLE}...")
        os.makedirs(BASE_DIR, exist_ok=True)
        snapshot_download(
            repo_id=HF_REPO,
            allow_patterns=[f"onnx/{BUNDLE}/*"],
            local_dir=BASE_DIR,
        )


PRECISION_SUFFIXES = {"int8": "_int8.onnx", "fp32": ".onnx"}
QUALITY_PRECISION_MAP = {"low": "int8", "high": "fp32"}


def _model_path(stem, precision="int8"):
    preferred = PRECISION_SUFFIXES[precision]
    fallback = PRECISION_SUFFIXES["fp32" if precision == "int8" else "int8"]
    for suffix in (preferred, fallback):
        path = os.path.join(BUNDLE_DIR, f"{stem}{suffix}")
        if os.path.exists(path):
            return path
    raise FileNotFoundError(f"No model file found for {stem} in {BUNDLE_DIR}")


def _pocket_tts_worker(input_queue, output_queue, precision="int8"):
    """Runs in a separate process. Loads models, generates audio."""
    try:
        import sentencepiece as spm

        _ensure_bundle()

        meta = json.loads(open(os.path.join(BUNDLE_DIR, "bundle.json")).read())
        sr = int(meta["sample_rate"])
        fr = float(meta["frame_rate"])
        ld = int(meta["latent_dim"])
        cd = int(meta["conditioning_dim"])
        fm = meta["flow_lm_state_manifest"]
        mm = meta["mimi_state_manifest"]

        tok = spm.SentencePieceProcessor()
        tok.Load(os.path.join(BUNDLE_DIR, meta["tokenizer_file"]))

        insert_bos = bool(meta.get("insert_bos_before_voice", False))
        bos_before_voice = None
        bos_file = meta.get("bos_before_voice_file")
        if insert_bos and bos_file:
            bos_before_voice = np.load(os.path.join(BUNDLE_DIR, bos_file)).astype(np.float32)

        opts = ort.SessionOptions()
        opts.intra_op_num_threads = 2
        opts.inter_op_num_threads = 1

        print(f"[PocketTTS-Worker] Loading models ({precision})...")
        tc = ort.InferenceSession(os.path.join(BUNDLE_DIR, "text_conditioner.onnx"), opts)
        lm = ort.InferenceSession(_model_path("flow_lm_main", precision), opts)
        lf = ort.InferenceSession(_model_path("flow_lm_flow", precision), opts)
        md = ort.InferenceSession(_model_path("mimi_decoder", precision), opts)
        print("[PocketTTS-Worker] Ready.")

        output_queue.put(("READY", None))

        while True:
            try:
                task = input_queue.get(timeout=1)
            except queue.Empty:
                continue

            if task is None:
                break

            text, voice, speed = task

            # --- Resolve voice state ---
            voice_path = os.path.join(BASE_DIR, "voices", f"{voice}.safetensors")
            if os.path.exists(voice_path):
                base_state = _load_voice_state(voice_path, fm)
            elif os.path.exists(voice):
                base_state = _load_voice_state(voice, fm)
            else:
                output_queue.put(("ERROR", f"Voice '{voice}' not found"))
                continue

            # --- Sentence-level generation ---
            chunks = _split_into_best_sentences(text, tok)

            for sentence in chunks:

                ids = np.array(tok.Encode(sentence), dtype=np.int64).reshape(1, -1)
                te = tc.run(None, {"token_ids": ids})[0]
                if te.ndim == 2:
                    te = te[None]

                state = {k: v.copy() for k, v in base_state.items()}
                r = lm.run(None, {
                    "sequence": np.zeros((1, 0, ld), dtype=np.float32),
                    "text_embeddings": te, **state,
                })
                _update(state, r, fm, 2)

                max_len = int(math.ceil(len(sentence.split()) / 3.0 * fr + 2.0 * fr))
                curr = np.full((1, 1, ld), np.nan, dtype=np.float32)
                empty_t = np.zeros((1, 0, cd), dtype=np.float32)
                eos_step = None
                latents = []

                for step in range(max_len):
                    r = lm.run(None, {"sequence": curr, "text_embeddings": empty_t, **state})
                    cond, eos_logit = r[0], r[1]
                    _update(state, r, fm, 2)

                    if eos_logit[0, 0] > -4.0 and eos_step is None:
                        eos_step = step
                    if eos_step is not None and step >= eos_step + 3:
                        break

                    x = np.random.normal(0.0, math.sqrt(0.7), (1, ld)).astype(np.float32)
                    flow = lf.run(None, {
                        "c": cond,
                        "s": np.array([[0.0]], dtype=np.float32),
                        "t": np.array([[1.0]], dtype=np.float32),
                        "x": x,
                    })[0]
                    latents.append((x + flow).reshape(1, 1, ld))
                    curr = latents[-1]

                if not latents:
                    continue

                full = np.concatenate(latents, axis=1)

                ms = _init_state(mm)
                audio_chunks = []
                for i in range(0, full.shape[1], 15):
                    r = md.run(None, {"latent": full[:, i:i+15, :], **ms})
                    audio_chunks.append(r[0].reshape(-1))
                    _update(ms, r, mm, 1)

                audio = np.concatenate(audio_chunks)
                audio = _change_speed(audio, speed)
                output_queue.put(("AUDIO", (audio, sr, sentence)))

            output_queue.put(("DONE", None))

    except Exception as e:
        import traceback
        output_queue.put(("ERROR", f"{e}\n{traceback.format_exc()}"))


class PocketTTSEngine(TTSEngine):
    def __init__(self, precision="int8"):
        if precision not in ("int8", "fp32"):
            raise ValueError(f"precision must be 'int8' or 'fp32', got '{precision}'")
        self.precision = precision
        self.process = None
        self.input_queue = None
        self.output_queue = None

    def load(self):
        if self.process and self.process.is_alive():
            return

        self.input_queue = multiprocessing.Queue()
        self.output_queue = multiprocessing.Queue()

        self.process = multiprocessing.Process(
            target=_pocket_tts_worker,
            args=(self.input_queue, self.output_queue, self.precision),
        )
        self.process.start()

        try:
            msg_type, payload = self.output_queue.get(timeout=30)
            if msg_type == "ERROR":
                raise RuntimeError(payload)
        except queue.Empty:
            self.unload()
            raise RuntimeError("PocketTTS Worker failed to start (Timeout)")

    def unload(self):
        if self.process:
            print("[PocketTTS] Killing Worker Process...")
            try:
                self.input_queue.put(None)
                self.process.join(timeout=1)
            except Exception:
                pass
            if self.process.is_alive():
                self.process.terminate()
                self.process.join()
            self.process = None
            self.input_queue = None
            self.output_queue = None
            print("[PocketTTS] Worker Dead. RAM Reclaimed.")

    def set_precision(self, precision):
        if precision == self.precision:
            return
        if precision not in ("int8", "fp32"):
            raise ValueError(f"precision must be 'int8' or 'fp32', got '{precision}'")
        self.unload()
        self.precision = precision

    def set_quality(self, quality):
        if quality not in ("low", "high"):
            raise ValueError(f"quality must be 'low' or 'high', got '{quality}'")
        self.set_precision(QUALITY_PRECISION_MAP[quality])

    def generate(self, text, voice, speed):
        if not self.process or not self.process.is_alive():
            self.load()

        self.input_queue.put((text, voice, speed))

        while True:
            try:
                msg_type, payload = self.output_queue.get(timeout=30)
                if msg_type == "AUDIO":
                    audio, sr, sentence = payload
                    yield (audio, sr, sentence)
                elif msg_type == "DONE":
                    break
                elif msg_type == "ERROR":
                    print(f"[PocketTTS] Worker Error: {payload}")
                    break
            except queue.Empty:
                print("[PocketTTS] Worker timed out generating audio.")
                break
