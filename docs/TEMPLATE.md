# Template conventions

Everything about this repository's structure, tooling, and workflow that's
identical across every instance of this template — as opposed to the root
`README.md`'s short, instance-owned preface. See `../CLAUDE.md`'s "Keeping
this file current" for where a new convention belongs.

## Contents

- `.devcontainer/` — the devcontainer setup; see its `README.md`. Its
  `stack/` directory holds one subdirectory per backing service
  (Postgres, Redis, S3/RustFS, Keycloak, MQTT, Selenium); see
  `stack/README.md`'s "Devcontainer stack pattern".
- `.github/` — CI and release workflows; see its `CONTENTS.md`.
- `.vscode/` — editor settings, tasks; see its `README.md`.
- `.claude/` — Claude Code CLI project config; see its `README.md`.
- `.mcp.json` — project-scope MCP servers not covered by a
  `.claude/settings.json` plugin; see `.claude/README.md`.
- `src/` — the application source (a `[lib]` + `[[bin]]` crate,
  `template_axum`); see its `README.md` for the module layering.
- `tests/` — integration/e2e-equivalent/perf tests that link against
  `src/`'s library target; unit tests stay colocated in `src/` as
  `#[cfg(test)] mod tests` blocks. See `tests/README.md`.
- `migration/` (under `src/`) — SeaORM migrations, applied automatically
  at startup; see `src/README.md`'s "Migrations".
- `scripts/` — the Dockerfile's per-stage setup scripts (`develop.sh`,
  `runner-setup.sh`); see `scripts/README.md`.
- `docs/` — knowledge about what the app does; this file is the one
  exception, documenting the template itself rather than product/domain
  knowledge.
- `.secrets/` — local secret files, never committed; see its `README.md`.
- `Dockerfile` — three build stages: `develop` (devcontainer), `builder`,
  `runner` (the release artifact).
- `compose.yml` — the release smoke-test stack (`NFR-0010`), distinct
  from `.devcontainer/compose.yml`; see `.github/workflows/README.md`'s
  "Issue moderation"-adjacent release notes and `compose.yml`'s own
  header comment for which backing services it includes and why.
- `Cargo.toml` / `Cargo.lock` — dependencies, pinned to exact versions;
  Renovate bumps them one at a time.
- `Makefile` — the release contract `release.yml` drives
  (`build`/`sbom`/`release-assets`/`publish`); see "Release: a Makefile
  contract" below.
- `.pre-commit-config.yaml` — git hooks, run by `prek` or `pre-commit`.
- `CLAUDE.md` — the AI-assisted coding workflow Claude Code follows in
  this repository; general conventions live in this file and each
  directory's own `README.md` instead.

## Getting started

1. Open this folder in a devcontainer (VS Code: "Reopen in Container" —
   `.vscode/extensions.json` recommends the extension that offers this —
   or any tool that reads `.devcontainer/devcontainer.json`). This starts
   the app alongside Postgres, Redis, S3 (RustFS), Keycloak (OIDC), MQTT,
   and a Selenium container Playwright can drive remotely; builds the
   `develop` stage; and installs the git hooks, all via
   `postCreateCommand`.
2. Run the app: `cargo run` (binary name `template-axum`), or attach a
   debugger to the same command. Startup applies any pending SeaORM
   migrations automatically, before the server starts accepting requests
   — see `src/README.md`'s "Migrations".
3. Health check: `curl localhost:8000/health/live` (liveness) or
   `curl localhost:8000/health/ready` (readiness — checks Postgres,
   Redis, S3, and the OIDC provider).
   `curl localhost:8000/crud/v1/heroes/v2/json` is the worked example
   CRUD resource (see `src/README.md`'s "Example CRUD resource: Hero").
   Writes need a bearer token from Keycloak — see
   `.devcontainer/stack/keycloak/README.md` — or, under
   `MODE=mock`/`ALLOW_MOCK_MODE=1`, `POST /mock/token` mints one without
   Keycloak at all (see `src/README.md`'s "MODE" section).

Without a devcontainer: install the pinned Rust toolchain (`Dockerfile`'s
`RUST_VERSION` ARG) via [`rustup`](https://rustup.rs/), export the
`POSTGRES_*` / `S3_ENDPOINT_URL` / `RUSTFS_ACCESS_KEY` /
`RUSTFS_SECRET_KEY` / `REDIS_URL` / `OIDC_ISSUER_URL` /
`OIDC_AUTHORIZATION_URL` / `OIDC_TOKEN_URL` / `OIDC_CLIENT_ID` /
`OIDC_AUDIENCE` variables `src/config.rs` reads (or run under
`MODE=mock`, `ALLOW_MOCK_MODE=1` for zero infrastructure), then
`cargo run` the same way. Install [`prek`](https://prek.j178.dev/) and
run `prek install` once to enable the git hooks.

## Checks

`.pre-commit-config.yaml` defines whitespace/EOF fixers, YAML/TOML/JSON
checks, `conventional-pre-commit` (Conventional Commits, enforced at the
`commit-msg` stage), the `template-sync-manifest` completeness check, and
this instance's own Rust hooks: `cargo fmt --check` and `cargo clippy
--all-targets -- -D warnings` (both on every commit); `cargo check
--all-targets`, `cargo test` (unit tier colocated in `src/`, plus the
integration/e2e-equivalent tiers in `tests/` — see `tests/README.md`),
`cargo llvm-cov` (a 97% line-coverage floor — see
`docs/adrs/0010-80-percent-line-coverage-floor-via-cargo-llvm-cov.md` and
`docs/nfrs/NFR-0023-test-coverage-gate.md`), and `cargo audit`
(dependency vulnerabilities) on push only, since they need the
devcontainer stack's real backing services and take longer to run.
`.github/renovate.json` opens a weekly update PR for Rust dependencies,
GitHub Actions, and every Dockerfile/compose image tag.

Run everything at once with:

```bash
prek run --all-files --hook-stage manual
```

Every hook except the commit-message check carries the `manual` stage,
so the command above runs everything else regardless of which git hook
would normally trigger it. Commit messages can only be checked by
actually committing (see the comment in `.pre-commit-config.yaml`).

If a lint rule produces a false positive, silence that one line with a
justified `#[allow(clippy::<lint>)]` (with a comment explaining why)
rather than loosening the project-wide `clippy` configuration or
disabling the lint entirely.

CI (`.github/workflows/checks.yml`) runs the same `--hook-stage manual`
command, inside the devcontainer itself, on every push and pull request
— as an `amd64`/`arm64` matrix, both legs native (no QEMU; see
`.github/workflows/README.md`'s "Architecture matrix"). `perf.yml` runs
the `tests/perf` `goose` load test against the built `runner` image on
`workflow_dispatch` (not per-PR — see
`docs/adrs/0018-goose-for-load-testing-not-locust.md`).
`.github/workflows/release.yml` is triggered manually to cut an
alpha/beta/rc/full release — see "Release: a Makefile contract" below
and `.github/workflows/README.md`.

## Release: a Makefile contract

`release.yml` computes the next SemVer tag (`compute_next_version.py`,
unchanged across every instance), runs `make build`, then the release
smoke-test gate (`NFR-0010`: boots the built `runner` image against
`compose.yml`'s real Postgres/Redis/S3/Keycloak stack and polls
`/health/ready`), then:

- `make build` — `docker build --target runner`, producing the OCI
  `runner` image.
- `make sbom` — an SPDX SBOM for that image via Syft, run against the
  local Docker daemon.
- `make release-assets` — populates `dist/` (gitignored) with the saved
  image tarball and its SBOM; `release.yml` just globs that directory,
  so it never needs to know the exact filenames.
- `make publish` — tags and pushes the image to `OCI_REGISTRY` if that
  repository/organization variable is set, skipping cleanly otherwise
  (see `.github/workflows/README.md`'s "OCI registry" section).

`release.yml` then runs `gh release create` with everything found in
`dist/`. Two environment variables are available to every target:
`RELEASE_VERSION` (e.g. `1.2.3` or `1.2.3-alpha.1`) and `RELEASE_TAG`
(the same, `v`-prefixed) — the `Makefile` uses `RELEASE_VERSION` to tag
the image and name `dist/`'s files.

This instance's `Makefile` is not template-owned (it's the whole point
that it differs per artifact type), so it's tracked like any other
instance-owned file — untouched by this template's own sync manifest.

## Template sync

Once instantiated, a repo created from this template can pull in later
template fixes/improvements via `.github/workflows/template-sync.yml`
(itself template-owned, `replace`-tier): on a schedule (cadence set by
the `TEMPLATE_SYNC_INTERVAL` repository variable, `weekly` or `monthly`)
or on demand, it diffs the instance against the template's latest tagged
release, per `.github/template-sync-manifest.yml`'s three tiers
(`replace`, `ignore`, `merge` — see that file's header), and opens a PR
with the result. It never pushes directly or auto-merges; a genuine
`merge`-tier conflict is left with `<<<<<<<` markers for a human to
resolve. An instance that predates this workflow bootstraps its
`.github/template-sync-state.json` via the workflow's manual
`initial_sync_tag`/`template_repo` inputs first.

A repo can itself be both an instance of `template-base` *and* its own
template for further instances (e.g. `template-fastapi`, the Python
counterpart this repo mirrors): its own
`.github/template-sync-manifest.yml` classifies its *own* tracked files
for *its* downstream instances, entirely separate from this template's
manifest.

## Versions and config

Every version and config value is defined in exactly one place; nothing
duplicates or re-pins it elsewhere:

- Rust toolchain version: the `RUST_VERSION` `ARG` default at the top of
  the `Dockerfile`. `builder`/`runner` pull it directly as the official
  `rust`/`debian` image tags; `develop` (based on the generic
  `mcr.microsoft.com/devcontainers/base` image, which carries no Rust of
  its own) has `scripts/develop.sh` install that same version via
  `rustup` (pinned itself via `RUSTUP_VERSION`), so all three stages
  compile with an identical compiler.
- Rust crate versions: `Cargo.toml` / `Cargo.lock`, pinned to exact
  versions — use `cargo add <crate>` / `cargo update` rather than editing
  the dependency lists by hand.
- Everything else pinned (base images, Actions, hook revisions, `prek`/
  Claude Code CLI/`snip`/`uv`/Python/Node.js/`cargo-audit`/
  `cargo-cyclonedx`/`cargo-llvm-cov` versions): pinned once, at its
  single point of use, to an exact patch version — never a floating
  range or `latest` — so Renovate can bump them one at a time and the
  diff shows exactly what changed. `uv`/`python3`/Node.js are
  infrastructure tooling only, not this app's runtime — see
  `Dockerfile`'s own comment: `uv` installs the pinned CPython build
  `prek` needs for any `language: python` hook and `.github/scripts/
  *.py`; Node.js's `npx` is only for the `clear-thought`/`playwright`
  MCP servers in `.mcp.json`.

`cargo-llvm-cov`'s coverage data and `target/` both live outside the
bind-mounted workspace (`CARGO_TARGET_DIR`, set in the devcontainer) —
same reasoning as not bind-mounting a Python venv would be: a build
directory inside the bind-mounted `/workspace` gets scanned file-by-file
by host antivirus/malware tools on Windows and is painfully slow to
write to. A new tool with its own on-disk cache follows the same
pattern.

## Code style

Every file gets a brief header stating what the file *is for* — one
line, sometimes two: a leading comment for most formats, a module `//!`
doc comment for Rust modules. Never describe the file's contents in the
header; that's what reading the file is for. Markdown files' own
title/opening line already serves this purpose. A format with no comment
syntax (`.json`) documents itself via the directory's `README.md`
instead.

A comment earns its place by saying something the code/config next to it
can't: *why* it's written this way, a non-obvious consequence, or a
constraint that isn't visible locally. Don't add a comment that just
restates what the following line already says in code — if removing a
comment loses no information, remove it. Every existing comment in this
repository follows this rule; keep new ones held to the same bar.

## Do

- Read `CLAUDE.md` and the `README.md` of every directory on the path to
  whatever you're changing before you change it.
- Open this repo in the devcontainer rather than assembling the
  toolchain by hand — it's the one environment this template guarantees.

## Don't

- Commit a `.env` file, or read one from application code — configuration
  lives in the compose files; secrets live in `.secrets/`.
- Add a `ports:` mapping or a `networks:` block to any file under
  `.devcontainer/` — see `.devcontainer/stack/README.md`'s "Devcontainer
  stack pattern" section for how host access and inter-service
  networking are handled instead.
- Assume a language runtime, backing service, or release artifact exists
  beyond what this instance itself defines — a fresh instance of
  `template-base` carries none of them.
