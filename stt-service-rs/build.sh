#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"
mkdir -p dist
cargo build --release --quiet
cp target/release/neuro-stt-service dist/neuro-stt-service

echo "Build Complete: dist/neuro-stt-service"
