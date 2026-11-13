#!/usr/bin/env bash
# Runner-stage hook (see README.md here): installs the OpenSSL runtime
# library. craftainer-crud's `mqtt-tls` feature links the system OpenSSL
# (rumqttc `use-native-tls`), which debian-slim does not ship. The builder
# stage's libssl-dev (scripts/builder.sh) is the matching build side. The
# package is libssl3t64 on the Dockerfile's Debian (trixie); `libssl3` is only
# a virtual name for it there.
set -euo pipefail

apt-get update
apt-get install -y --no-install-recommends libssl3t64
apt-get clean
rm -rf /var/lib/apt/lists/*
