# scripts/

Setup logic for each `Dockerfile` RUN step — every RUN in the Dockerfile
calls exactly one of these, never a binary directly:

- `develop.sh` — the `develop` stage: apt packages, rustup (the pinned Rust
  toolchain), `cargo-audit`/`cargo-cyclonedx`, the Claude Code CLI, `snip`,
  plus the same psql/redis-cli/`rc`/`kcadm`/uv/Node.js infrastructure
  tooling template-fastapi's own `develop.sh` installs — needed to talk to
  this instance's own backing-services stack (see `.devcontainer/stack/`).
- `builder.sh` — the `builder` stage's tooling setup (apt packages only;
  Rust/cargo already come from the stage's own `rust:` base image).
- `builder-sync-deps.sh` / `builder-sync-app.sh` — the `builder` stage's two
  `cargo build --release` steps, split so a source-only change doesn't
  invalidate the dependency-compilation layer: `builder-sync-deps.sh`
  builds against a dummy `main.rs`, `builder-sync-app.sh` builds the real
  one against those already-compiled dependencies.
- `runner-setup.sh` — the `runner` stage's user/permission/CA-certificates
  setup.
- `runner.sh` — the `runner` stage's entrypoint (starts the compiled
  binary).

## Do

- Keep each script runnable and idempotent on its own (`bash
  scripts/develop.sh <args>`) so you can debug a stage without a full
  build.
- Take a version as a script argument (see `develop.sh`) rather than
  hardcoding it, when the Dockerfile already defines it as an `ARG` —
  keeps that `ARG` the single source of truth.
- Clean up apt lists / caches at the end of a script that installs
  packages, to keep the resulting layer small.

## Don't

- Invoke a binary directly from a Dockerfile `RUN` — add or extend a
  script here instead.
- Install tooling a different stage needs — `runner.sh` in particular
  should stay a plain entrypoint, not a setup script.
