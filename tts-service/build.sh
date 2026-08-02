#!/bin/bash

SITE_PACKAGES=$(uv run python -c "import site; print(site.getsitepackages()[0])")

uv run python -m nuitka --onefile \
  --include-data-files="$SITE_PACKAGES/kokoro_onnx/config.json"=kokoro_onnx/config.json \
  --include-data-files="$SITE_PACKAGES/language_tags/data/json/index.json"=language_tags/data/json/index.json \
  --include-data-files="$SITE_PACKAGES/language_tags/data/json/registry.json"=language_tags/data/json/registry.json \
  --include-data-dir="$SITE_PACKAGES/espeakng_loader/espeak-ng-data"=espeakng_loader/espeak-ng-data \
  --include-data-files="$SITE_PACKAGES/espeakng_loader/libespeak-ng.so"=espeakng_loader/libespeak-ng.so \
  --include-distribution-metadata=huggingface_hub \
  --include-distribution-metadata=kokoro-onnx \
  --include-distribution-metadata=onnxruntime \
  --include-distribution-metadata=safetensors \
  --include-distribution-metadata=sentencepiece \
  --output-dir=dist \
  --output-filename=neuro-tts-service \
  --assume-yes-for-downloads \
  --lto=yes \
  --no-deployment-flag=self-execution \
  --python-flag=no_site \
  src/tts_service.py

echo "Build Complete!"
