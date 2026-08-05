#!/bin/bash
# Downloads the Parakeet TDT v3 ONNX model files used by neuro-stt-service.
set -euo pipefail

MODEL_DIR="${1:-$HOME/.local/share/neuropipe/stt/parakeet-v3}"
QUANT="${QUANT:-int8}"  # int8 (default) | fp32
VAD_DIR="${VAD_DIR:-$HOME/.local/share/neuropipe/stt}"

BASE="https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main"
mkdir -p "$MODEL_DIR"

echo "Downloading parakeet-tdt-0.6b-v3 ($QUANT) -> $MODEL_DIR"

curl -fL --progress-bar "$BASE/config.json" -o "$MODEL_DIR/config.json"
curl -fL --progress-bar "$BASE/vocab.txt" -o "$MODEL_DIR/vocab.txt"

if [ "$QUANT" = "int8" ]; then
  curl -fL --progress-bar "$BASE/encoder-model.int8.onnx" -o "$MODEL_DIR/encoder-model.int8.onnx"
  curl -fL --progress-bar "$BASE/decoder_joint-model.int8.onnx" -o "$MODEL_DIR/decoder_joint-model.int8.onnx"
else
  curl -fL --progress-bar "$BASE/encoder-model.onnx" -o "$MODEL_DIR/encoder-model.onnx"
  curl -fL --progress-bar "$BASE/encoder-model.onnx.data" -o "$MODEL_DIR/encoder-model.onnx.data"
  curl -fL --progress-bar "$BASE/decoder_joint-model.onnx" -o "$MODEL_DIR/decoder_joint-model.onnx"
fi

echo "Done."
echo ""
echo "Downloading Silero VAD model -> $VAD_DIR"
mkdir -p "$VAD_DIR"
curl -fL --progress-bar \
  "https://raw.githubusercontent.com/snakers4/silero-vad/master/src/silero_vad/data/silero_vad.onnx" \
  -o "$VAD_DIR/silero_vad.onnx"
echo "All done."
