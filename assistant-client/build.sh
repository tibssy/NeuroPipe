#!/bin/bash

uv run python -m nuitka --onefile \
  --output-dir=dist \
  --output-filename=neuro-assistant-client \
  --assume-yes-for-downloads \
  --lto=yes \
  --python-flag=no_site \
  src/assistant_client.py

echo "Build Complete!"
