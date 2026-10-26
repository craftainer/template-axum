#!/usr/bin/env bash
# Real-stack perf run (tests/perf/README.md, docs/adrs/0018 addendum):
# starts the runner image against the devcontainer stack's own Postgres,
# Redis, MQTT and Keycloak (MODE=dev, real token validation), seeds a fresh
# `ax4h_perf` database through the app's API, drives the goose
# `HeroJourneyReal` scenario, then stops what it started.
#
# Usage: tests/perf/run-real-stack.sh <image> [goose args...]
#   e.g. tests/perf/run-real-stack.sh template-axum:ax4h --users 20 --run-time 60s
#
# Prerequisites: the stack containers `postgres-postgres-1`, `redis-redis-1`,
# `mqtt-mqtt-1` and `keycloak-keycloak-1` are running (one compose project
# per fragment under .devcontainer/stack/, so each has its own network).
# Environment overrides: PERF_DB (default ax4h_perf), PERF_SEED (records to
# seed, rounded up to a multiple of 4, default 500), PERF_TOOLCHAIN_IMAGE (a rust image with cargo).
# App settings under test: any of DB_POOL_SIZE, DB_POOL_MIN_SIZE,
# DB_POOL_RECYCLE, DB_POOL_IDLE_TIMEOUT, DB_CONNECT_TIMEOUT and
# RUN_MIGRATIONS_ON_STARTUP that is set in the caller's environment is passed to the app
# container (docs/adrs/0022); unset ones leave the app's defaults.
set -euo pipefail

image=${1:?usage: $0 <image> [goose args...]}
shift

repo=$(cd "$(dirname "$0")/../.." && pwd)
db=${PERF_DB:-ax4h_perf}
seed=${PERF_SEED:-500}
toolchain=${PERF_TOOLCHAIN_IMAGE:-craftainer-tc/template-rust-crate:rc2}
app=ax4h-perf-app
cargo_cache=${PERF_CARGO_CACHE:-$HOME/.cache}

for c in postgres-postgres-1 redis-redis-1 mqtt-mqtt-1 keycloak-keycloak-1; do
  if [ "$(docker inspect -f '{{.State.Running}}' "$c" 2>/dev/null)" != true ]; then
    echo "required stack container $c is not running" >&2
    exit 1
  fi
done

for n in keycloak_default postgres_default redis_default mqtt_default; do
  if ! docker network inspect "$n" >/dev/null 2>&1; then
    echo "required docker network $n does not exist (stack started under another compose project name?)" >&2
    exit 1
  fi
done
if ! docker image inspect "$toolchain" >/dev/null 2>&1; then
  echo "toolchain image $toolchain is not available locally (set PERF_TOOLCHAIN_IMAGE)" >&2
  exit 1
fi
mkdir -p "$cargo_cache"

# Goose picks the scenario itself; a caller-supplied --scenarios would
# duplicate it.
for a in "$@"; do
  case "$a" in
    --scenarios | --scenarios=* | -s)
      echo "do not pass --scenarios; the script selects HeroJourneyReal" >&2
      exit 2
      ;;
  esac
done

cleanup() {
  docker rm -f "$app" >/dev/null 2>&1 || true
}
trap cleanup EXIT

# A fresh database owned by this tier only. The name is fixed above, never
# derived from input, and no other database is touched.
psql() { docker exec postgres-postgres-1 psql -v ON_ERROR_STOP=1 -U app -d postgres "$@"; }
psql -qc "DROP DATABASE IF EXISTS ${db} WITH (FORCE)"
psql -qc "CREATE DATABASE ${db}"

# Pool settings under test (docs/adrs/0022): forward only those the caller set,
# so an unset variable leaves the app's default. POSTGRES_SSLMODE is not
# forwarded: the stack's Postgres has no TLS.
app_env=()
for var in DB_POOL_SIZE DB_POOL_MIN_SIZE DB_POOL_RECYCLE \
  DB_POOL_IDLE_TIMEOUT DB_CONNECT_TIMEOUT RUN_MIGRATIONS_ON_STARTUP; do
  if [ -n "${!var:-}" ]; then
    app_env+=(-e "$var=${!var}")
  fi
done

# The app joins keycloak_default (where goose also runs, so the token's
# issuer URL is the same http://keycloak:8080/... for both) and the Postgres,
# Redis and MQTT networks. Redis DB 14 keeps the rate-limit counters apart
# from anything else on the shared instance. The per-minute limits are
# raised so the limiter does not turn the load into 429s.
docker rm -f "$app" >/dev/null 2>&1 || true
docker create --name "$app" --network keycloak_default \
  -e MODE=dev \
  -e POSTGRES_HOST=postgres -e POSTGRES_USER=app -e POSTGRES_PASSWORD=app \
  -e POSTGRES_DB="$db" \
  -e REDIS_URL=redis://redis:6379/14 \
  -e MQTT_HOST=mqtt -e MQTT_PORT=1883 \
  -e OIDC_ISSUER_URL=http://keycloak:8080/realms/template-axum \
  -e OIDC_AUDIENCE=api \
  -e RATE_LIMIT_HERO_WRITE_PER_MINUTE=1000000 \
  -e RATE_LIMIT_MOCK_TOKEN_PER_MINUTE=1000000 \
  ${app_env[@]+"${app_env[@]}"} \
  "$image" >/dev/null
docker network connect postgres_default "$app"
docker network connect redis_default "$app"
docker network connect mqtt_default "$app"
docker start "$app" >/dev/null

for _ in $(seq 1 60); do
  if docker run --rm --network keycloak_default "$toolchain" \
    curl --fail --silent --max-time 3 "http://$app:8000/health/live" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 2
done
if [ -z "${ready:-}" ]; then
  echo "the app never became healthy" >&2
  docker logs "$app" >&2 || true
  exit 1
fi

goose() {
  docker run --rm --network keycloak_default \
    -u "$(id -u):$(id -g)" \
    -e CARGO_HOME=/home/vscode/.cache/cargo-home \
    -e CARGO_TARGET_DIR=/home/vscode/.cache/cargo-target-perf \
    -e PERF_TARGET=real \
    -v "$cargo_cache:/home/vscode/.cache" \
    -v "$repo:$repo" -w "$repo" \
    "$toolchain" \
    cargo run -p template-axum-perf --release -- \
    --host "http://$app:8000" --no-telnet --no-websocket "$@"
}

# Seed through the API (4 users x seed/4 iterations of one create each),
# then run the journey. Seeding is not part of the measured run.
if [ "$seed" -gt 0 ]; then
  per_user=$(((seed + 3) / 4)) # rounded up; never 0, which goose reads as unlimited
  goose --scenarios HeroSeedReal --users 4 --iterations "$per_user" \
    --no-print-metrics --quiet
fi
goose --scenarios HeroJourneyReal "$@"
