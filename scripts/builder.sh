#!/usr/bin/env bash
# Sets up the `builder` stage's build tooling. Rust/cargo already come from
# this stage's own `rust:${RUST_VERSION}-slim-${DEBIAN_VERSION}` base image
# (see the Dockerfile) -- this only adds what a handful of dependencies'
# `-sys` crates need to link against at build time.
set -euo pipefail

apt-get update
apt-get install -y --no-install-recommends \
    ca-certificates \
    pkg-config \
    libssl-dev

apt-get clean
rm -rf /var/lib/apt/lists/*
