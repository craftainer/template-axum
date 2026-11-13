#!/usr/bin/env bash
# Runner-stage hook (see README.md here): installs the OpenSSL runtime
# library. craftainer-crud's `mqtt-tls` feature links the system OpenSSL
# (rumqttc `use-native-tls`), which debian-slim does not ship. The builder
# stage's libssl-dev (scripts/builder.sh) is the matching build side.
set -euo pipefail

apt-get update
apt-get install -y --no-install-recommends libssl3
apt-get clean
rm -rf /var/lib/apt/lists/*
