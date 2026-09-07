#!/usr/bin/env bash
# Prepares the runner stage's runtime user for arbitrary-UID platforms
# (e.g. OpenShift, which ignores an image's own UID and always runs it
# under group 0): group-0 ownership plus group read/write/execute on
# every directory the app needs, and a numeric (not named) final USER in
# the Dockerfile. The compiled binary gets the equivalent treatment via
# COPY --chown/--chmod instead of a recursive chmod here.
set -euo pipefail

app_uid=$1

# ca-certificates: debian-slim ships none by default, but the app needs a
# trust store to validate TLS to Postgres/Redis/S3/the OIDC provider (phase
# 2) -- installed here, not builder.sh, since only the runner stage needs it
# at runtime.
apt-get update
apt-get install -y --no-install-recommends ca-certificates
apt-get clean
rm -rf /var/lib/apt/lists/*

chmod +x /usr/local/bin/runner.sh
useradd --create-home --uid "$app_uid" --gid 0 appuser

chgrp -R 0 /app /home/appuser
chmod -R g+rwX /app /home/appuser
