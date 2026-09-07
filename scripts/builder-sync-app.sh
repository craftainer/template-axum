#!/usr/bin/env bash
# Builds the real application on top of the dependencies
# builder-sync-deps.sh already compiled, then copies the binary out of the
# cache-mounted target/ dir (see the Dockerfile) into a plain path the
# runner stage's COPY --from=builder can reach -- a cache mount's contents
# don't persist into the final image layer otherwise.
set -euo pipefail

cargo build --release --locked
mkdir -p /build/bin
cp target/release/template-axum /build/bin/template-axum
