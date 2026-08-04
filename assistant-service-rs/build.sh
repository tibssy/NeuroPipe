#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"
mkdir -p dist
cargo build --release --quiet
cp target/release/neuro-assistant-service dist/neuro-assistant-service

echo "Build Complete: dist/neuro-assistant-service"