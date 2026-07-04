#!/usr/bin/env bash
# Ensure the system rustup (standard ~/.rustup / ~/.cargo locations) has the
# stable toolchain plus everything this repo needs. Idempotent and cheap when
# already installed. In CI, rustup itself is provided by a setup action.
set -euo pipefail

if ! command -v rustup >/dev/null 2>&1; then
    echo "error: rustup not found; install it from https://rustup.rs" >&2
    exit 1
fi

rustup toolchain install stable --profile minimal --component clippy --component rustfmt
# Zed extensions compile to wasm32-wasip1.
rustup target add --toolchain stable wasm32-wasip1
rustup run stable rustc --version
rustup run stable cargo --version
