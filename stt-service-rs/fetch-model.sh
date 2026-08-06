#!/bin/bash
# Downloads the Parakeet TDT v3 ONNX model files used by neuro-stt-service.
set -euo pipefail

MODEL_DIR="${1:-$HOME/.local/share/neuropipe/stt/parakeet-v3}"
VAD_DIR="${VAD_DIR:-$HOME/.local/share/neuropipe/stt}"

BASE="https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main"
mkdir -p "$MODEL_DIR"

echo "Downloading Parakeet TDT v3 int8 model -> $MODEL_DIR"

curl -fL --progress-bar "$BASE/config.json" -o "$MODEL_DIR/config.json"
curl -fL --progress-bar "$BASE/vocab.txt" -o "$MODEL_DIR/vocab.txt"

curl -fL --progress-bar "$BASE/encoder-model.int8.onnx" -o "$MODEL_DIR/encoder-model.int8.onnx"
curl -fL --progress-bar "$BASE/decoder_joint-model.int8.onnx" -o "$MODEL_DIR/decoder_joint-model.int8.onnx"

echo "Done."
echo ""
echo "Downloading Silero VAD model -> $VAD_DIR"
mkdir -p "$VAD_DIR"
# v6 export (onnx-community) — v5 ONNX from snakers4 under-scores TTS/real speech
curl -fL --progress-bar \
  "https://huggingface.co/onnx-community/silero-vad/resolve/main/onnx/model.onnx" \
  -o "$VAD_DIR/silero_vad.onnx"
echo "All done."
