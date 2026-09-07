#!/usr/bin/env bash
# Entrypoint for the `runner` stage: starts the application.
set -euo pipefail

exec /app/template-axum
