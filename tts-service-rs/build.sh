#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"
mkdir -p dist
cargo build --release --quiet
cp target/release/neuro-tts-service dist/neuro-tts-service

echo "Build Complete: dist/neuro-tts-service"
