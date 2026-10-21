#!/usr/bin/env bash
# Warms the cargo target dir (CARGO_TARGET_DIR, set in compose.instance.yml)
# so the first `cargo test`/`clippy` in a fresh devcontainer doesn't pay for
# compiling every dependency.
set -euo pipefail

cd /workspace
cargo build
