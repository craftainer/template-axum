# scripts/

- `develop.sh` — `../.devcontainer/Dockerfile`'s `develop`-stage setup: installs `prek`,
  the Claude Code CLI, `snip` and the GitHub CLI (`gh`), the only tooling this template requires
  regardless of instance language. An instance adds its own language
  runtime/tooling install on top, either by extending this script or
  copying its shape into a second script. Prefer the hook below over
  either.
- `post-create.sh` — the devcontainer's `postCreateCommand`: git
  `safe.directory`, `prek install`, then every `post-create.d/*.sh`.
- `develop.d/`, `post-create.d/`, `runner-setup.d/` — instance hook
  directories, empty apart from their own `README.md`; see
  `docs/TEMPLATE.md`'s "Instance extension points".
- `runner-setup.sh` — the top-level `Dockerfile`'s `runner` stage setup:
  group-0 runtime user for arbitrary-UID platforms (run before copying
  app code, which is then copied read-only), then every
  `runner-setup.d/*.sh`.
- `image-scan.sh` — the manual-stage `image-scan` hook: Trivy over the
  tars in `dist/`.
- `image-smoke.sh` — `make image-smoke` (a `release-check` prerequisite):
  converts the arch's image in the release OCI tar from `dist/` to a
  docker-archive with the pinned skopeo (`docker load` can't read a buildx
  OCI tar on the classic image store), loads and runs it, and polls
  `IMAGE_SMOKE_PATH` until 2xx. Needs `SKOPEO_IMAGE`, which the Makefile
  exports; usage in the script's header.
- `skopeo.sh` — runs the pinned skopeo image as the caller's user with
  `dist/` mounted at `/dist`; used by `make publish` and `image-smoke.sh`.
- `check-dockerfile-versions.sh ARG...` — fails when a named `ARG` default
  differs between `.devcontainer/Dockerfile` and the top-level `Dockerfile`.
- `check-rust-version.sh` — fails when the Rust pin in the top-level
  `Dockerfile` differs from `develop.d/10-rust.sh`'s, or from
  `Cargo.toml`'s `rust-version` (major.minor).
- `develop.d/10-rust.sh`, `develop.d/20-stack-tools.sh` — this instance's
  devcontainer tooling: the pinned Rust toolchain with `cargo-audit` and
  `cargo-llvm-cov`; then the CLIs for the backing-services stack
  (`psql`, `redis-cli`, `rc`, `kcadm`).
- `post-create.d/10-cargo-build.sh` — warms the cargo target dir.
- `runner-setup.d/10-ca-certificates.sh` — installs the runtime CA trust
  store.
- `builder.sh` — the `builder` stage's tooling setup (apt packages only;
  Rust/cargo come from the stage's own `rust:` base image).
- `builder-sync-deps.sh` / `builder-sync-app.sh` — the `builder` stage's two
  `cargo build --release` steps, split so a source-only change doesn't
  invalidate the dependency-compilation layer: `builder-sync-deps.sh`
  builds against a dummy `main.rs`, `builder-sync-app.sh` builds the real
  one against those already-compiled dependencies.
- `runner.sh` — the `runner` stage's entrypoint (starts the compiled
  binary).

## Do

- Keep this script's own tools pinned to an exact version (a `# renovate:`
  ARG in `../.devcontainer/Dockerfile`) and checksum-verified where the upstream project
  publishes checksums, matching the pattern each install step here already
  follows.

## Don't

- Add a language runtime or stack-specific tool here — that belongs in the
  instance's own layer (`develop.d/`), not the base template's.
