#!/bin/bash

uv run python -m nuitka --onefile \
  --output-dir=dist \
  --output-filename=neuro-tts-trigger \
  --lto=yes \
  --python-flag=no_site \
  src/tts_client.py