#!/bin/bash

SITE_PACKAGES=$(uv run python -c "import site; print(site.getsitepackages()[0])")

uv run python -m nuitka --onefile \
  --include-package-data=pysilero_vad \
  --include-data-files="$SITE_PACKAGES/pysilero_vad/*.bin"=pysilero_vad/ \
  --include-package-data=onnx_asr \
  --include-module=neuropipe_config \
  --output-dir=dist \
  --output-filename=neuro-stt-service \
  --assume-yes-for-downloads \
  --lto=yes \
  --python-flag=no_site \
  src/stt_service.py