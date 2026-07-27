#!/bin/bash

uv run python -m nuitka --onefile \
  --include-module=neuropipe_config \
  --output-dir=dist \
  --output-filename=neuro-assistant-service \
  --assume-yes-for-downloads \
  --lto=yes \
  --python-flag=no_site \
  src/assistant_service.py

echo "Build Complete!"
