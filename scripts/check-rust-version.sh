#!/usr/bin/env bash
# Usage: check-rust-version.sh
#
# The Rust toolchain is pinned in the runtime Dockerfile (RUST_VERSION ARG,
# builder stage) and in scripts/develop.d/10-rust.sh (devcontainer); they
# must be the same exact version, and Cargo.toml's rust-version its
# major.minor. Exits 1 on any mismatch.
set -euo pipefail

docker_v="$(sed -nE 's/^ARG RUST_VERSION=([^[:space:]]+).*/\1/p' Dockerfile | head -n1)"
hook_v="$(sed -nE 's/^RUST_VERSION=([^[:space:]]+).*/\1/p' scripts/develop.d/10-rust.sh | head -n1)"
msrv="$(sed -nE 's/^rust-version = "([^"]+)".*/\1/p' Cargo.toml | head -n1)"

status=0
if [[ -z "$docker_v" || -z "$hook_v" || -z "$msrv" ]]; then
    echo "could not read all three pins (Dockerfile='$docker_v', 10-rust.sh='$hook_v', Cargo.toml='$msrv')" >&2
    exit 1
fi
if [[ "$docker_v" != "$hook_v" ]]; then
    echo "RUST_VERSION mismatch: Dockerfile has '$docker_v', scripts/develop.d/10-rust.sh has '$hook_v'" >&2
    status=1
fi
if [[ "${docker_v%.*}" != "$msrv" ]]; then
    echo "rust-version mismatch: Cargo.toml has '$msrv', Dockerfile's RUST_VERSION is '$docker_v'" >&2
    status=1
fi
exit "$status"
