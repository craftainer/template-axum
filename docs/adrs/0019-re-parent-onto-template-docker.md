# 0019. Re-parent onto `template-docker` and bump Rust to 1.99.1

## Status

Accepted

## Context

`template-axum` carried its own image release `Makefile`, a three-stage
`Dockerfile` (`develop`, `builder`, `runner`) whose `develop` stage was the
devcontainer, a root `compose.yml` for the release smoke gate (`NFR-0010`)
and its own `scripts/develop.sh`. It had no template-sync state. The
shared image layer now lives in `template-docker` (its ADR 0001), which
fastapi, react and axum are to sync from, and which owns the release
`Makefile`, the devcontainer `Dockerfile`, the smoke script and the
arbitrary-UID runner setup. Rust 1.99.1 is the current stable release and
`template-rust-crate` already pins it.

## Decision

`template-axum` is an instance of `template-docker` v1.0.0
(`.github/template-sync-state.json`) and stays a template for further
instances; its `.github/template-sync-manifest.yml` starts from
template-docker's and adds axum's paths.

**What moved**

- The devcontainer image is template-docker's `.devcontainer/Dockerfile`.
  Rust (rustup, `cargo-audit`, `cargo-llvm-cov`) moved from
  `scripts/develop.sh` to `scripts/develop.d/10-rust.sh`; the stack CLIs
  (`psql`, `redis-cli`, `rc`, `kcadm`) to `scripts/develop.d/20-stack-tools.sh`.
- The dev service's settings, the stack `include:` list and
  `CARGO_TARGET_DIR` moved to `.devcontainer/compose.instance.yml`; the
  `cargo build` warm-up to `scripts/post-create.d/10-cargo-build.sh`.
- The runtime CA bundle install moved to
  `scripts/runner-setup.d/10-ca-certificates.sh`; `scripts/runner-setup.sh`
  is template-docker's.
- The release `Makefile` is template-docker's; axum's values are in
  `image.mk`.

**What was deleted**

- The `develop` stage of the root `Dockerfile`, the old `Makefile` and the
  old `scripts/develop.sh` and `runner-setup.sh` bodies.
- The root `compose.yml` and the compose smoke step in `release.yml`
  (`release.yml` is now template-docker's).
- `cargo-cyclonedx` from the devcontainer: nothing used it (SBOMs are
  Syft's).

**`image.mk` values**: `DOCKERFILE := Dockerfile`, `TARGET := runner`;
`BUILD_ARGS` passes `APP_VERSION` and `APP_COMMIT`; `OCI_DESCRIPTION`;
`OCI_LICENSE := GPL-3.0-or-later`; `COVERAGE_REPORT := coverage.xml`;
`IMAGE_SMOKE_RUN_ARGS := -e MODE=mock -e ALLOW_MOCK_MODE=1` with
`/health/live`. `ARCHES` keeps the default `amd64 arm64`. `release-check`
gets two prerequisites: `coverage` (`cargo llvm-cov --cobertura` into
`dist/coverage.xml`) and `stack-smoke`.

**Smoke gate (`NFR-0010`)**: the real-services smoke runs as
`release-check: stack-smoke` inside the devcontainer (the image under
`MODE=dev`, `/health/ready`, the stack's `.env` files, and `--add-host`
entries because the devcontainer's Docker daemon cannot join the stack's
network). Each per-arch image is smoke-tested before its push in mock mode.
`NFR-0010` was amended: the gate precedes `finalize`, the tag and the
release, not the per-arch registry push.

**Rust**: 1.98.1 to 1.99.1 in `Dockerfile` (`RUST_VERSION`),
`scripts/develop.d/10-rust.sh` and `Cargo.toml` (`rust-version = "1.99"`).
`.devcontainer/Dockerfile` is template-owned and cannot declare
`RUST_VERSION`, so `scripts/check-rust-version.sh` (a hook) keeps the three
equal; `check-dockerfile-versions` covers `DEBIAN_VERSION` only.

**Builder fixes found by the first real `make build`**: the dependency-layer
script never created a stub `src/lib.rs` or the `tests/perf` workspace
member's target, so the root image build had been failing since the `[lib]`
target and the workspace were added; `builder-sync-deps.sh` and the
`Dockerfile` now provide both. `Cargo.lock` took patch updates of
`quinn-proto`, `rustls`, `rustls-webpki`, `webpki-root-certs` and
`webpki-roots` to clear `cargo audit` advisories.

## Consequences

- Behaviour changes for images: OCI labels (title, source, version,
  revision, created, licenses, SBOM and coverage-report URLs) are new; the
  image is built per arch as an OCI tarball and pushed with skopeo from
  `dist/`; the binary is mode 0550 (was 0750); `/app` and the home
  directory follow template-docker's group-0 layout; `APP_VERSION` and
  `APP_COMMIT` are in the environment.
- The dev service is named `myapp` (template-docker's `compose.yml` fixes
  it), not `api`; `selenium` targets `http://myapp:8000`. Everything under
  `.devcontainer/` changed, so the devcontainer must be rebuilt.
- A release's per-arch images are pushed before the real-services gate
  runs; a failing gate leaves `-<arch>` tags in the registry but no
  manifest list, tag or GitHub release.
- Rust is pinned in two files plus `Cargo.toml`, guarded by a hook rather
  than a single source.
- Further instances sync from axum, which syncs from template-docker, which
  syncs from base.
