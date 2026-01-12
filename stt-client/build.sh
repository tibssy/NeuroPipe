#!/bin/bash

uv run python -m nuitka --onefile \
  --output-dir=dist \
  --output-filename=neuro-stt-trigger \
  --lto=yes \
  --python-flag=no_site \
  src/trigger_input.py