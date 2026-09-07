#!/usr/bin/env bash
# Builds only the dependency graph against a dummy main.rs, so a
# source-only change (builder-sync-app.sh) doesn't invalidate this, far
# slower, layer -- see the Dockerfile builder stage's comment for why this
# dummy-main trick was chosen over cargo-chef's extra planner stage.
set -euo pipefail

mkdir -p src
echo 'fn main() {}' > src/main.rs
cargo build --release --locked

# Removes this crate's own compiled artifacts (but not its dependencies',
# which stay cached) so builder-sync-app.sh's real build doesn't mistake
# the dummy binary for already up to date once the real src/ lands.
rm -rf target/release/deps/template_axum-* target/release/template-axum
rm -f src/main.rs
