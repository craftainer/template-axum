#!/usr/bin/env bash
# Installs the OpenSSL development files the `mqtt-tls` feature of
# craftainer-crud builds against (rumqttc's `use-native-tls` -> native-tls ->
# openssl-sys needs pkg-config and libssl-dev). The release builder stage
# installs the same two packages in scripts/builder.sh.
# Runs as root during the `develop` stage build (see README.md here).
set -euo pipefail

apt-get update
apt-get install -y --no-install-recommends pkg-config libssl-dev
apt-get clean
rm -rf /var/lib/apt/lists/*
