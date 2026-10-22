#!/usr/bin/env bash
# Runner-stage hook (see README.md here): installs the CA trust store.
# debian-slim ships none, but the app needs one to validate TLS to
# Postgres/Redis/S3/the OIDC provider.
set -euo pipefail

apt-get update
apt-get install -y --no-install-recommends ca-certificates
apt-get clean
rm -rf /var/lib/apt/lists/*
