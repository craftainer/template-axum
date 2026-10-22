# Template conventions

Everything about this repository's structure, tooling, and workflow that's
identical across every instance of this template — as opposed to the root
`README.md`'s short, instance-owned preface. See `../CLAUDE.md`'s "Keeping
this file current" for where a new convention belongs.

`template-axum` is a template of its own: an instance of `template-docker`
(itself an instance of `template-base`) that adds a Rust/axum application
with a Postgres/Redis/S3/Keycloak/MQTT devcontainer stack. Further
instances are created from it; its own `.github/template-sync-manifest.yml`
classifies its tracked files for them (see "Template sync" below).

## Contents

- `.devcontainer/` — the devcontainer setup; see its `README.md`. Its
  `stack/` directory holds one subdirectory per backing service
  (Postgres, Redis, S3/RustFS, Keycloak, MQTT, Selenium); see
  `stack/README.md`'s "Devcontainer stack pattern". `compose.instance.yml`
  includes them and configures the dev service (`myapp`).
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
- `docs/` — knowledge about what the app does; this file is the one
  exception, documenting the template itself rather than product/domain
  knowledge.
- `.secrets/` — local secret files, never committed; see its `README.md`.
- `.devcontainer/Dockerfile` — a single `develop` stage (the devcontainer
  image), with its build-context excludes in
  `.devcontainer/Dockerfile.dockerignore`. The repo root has no
  `Dockerfile`: that one name is reserved for an instance's own runtime
  image (see "Instance extension points"), so a repo never has more than
  one top-level Dockerfile.
- `scripts/develop.sh`, `scripts/post-create.sh` — the `develop` stage's
  setup script and the devcontainer's `postCreateCommand`, each running
  an instance hook directory (`develop.d/`, `post-create.d/`); see
  `scripts/README.md` and "Instance extension points" below.
- `Makefile` — the release contract `release.yml` drives
  (`build`/`sbom`/`release-assets`/`publish`); in `template-docker` the
  image implementation — see "Release: a Makefile contract" and "Image
  layer (template-docker)" below.
- `Dockerfile` — two build stages: `builder` and `runner` (the release
  artifact); the devcontainer's `develop` stage is the template-owned
  `.devcontainer/Dockerfile`, with Rust installed by
  `scripts/develop.d/10-rust.sh`.
- `image.mk`, `.dockerignore`, `scripts/image-smoke.sh`,
  `scripts/runner-setup.sh`, `scripts/runner-setup.d/`,
  `scripts/check-dockerfile-versions.sh` — the image layer; see "Image
  layer (template-docker)" below. `image.mk` carries axum's values and the
  real-services smoke (`stack-smoke`, `NFR-0010`); `docs/adrs/0019`
  records the move onto the layer.
- `Cargo.toml` / `Cargo.lock` — dependencies, pinned to exact versions;
  Renovate bumps them one at a time.
- `.pre-commit-config.yaml` — git hooks, run by `prek` or `pre-commit`;
  plus this instance's Rust hooks.
- `CLAUDE.md` — the AI-assisted coding workflow Claude Code follows in
  this repository; general conventions live in this file and each
  directory's own `README.md` instead.

## Instance extension points

Base-owned files stay `replace` tier (a sync overwrites them) and expose
hooks an instance fills with files it owns, so an instance never edits a
base-owned file directly. Don't edit `.devcontainer/Dockerfile`, `scripts/develop.sh`
or `compose.yml` in an instance; use the hooks below.

| Hook | Base side (template-owned) | Instance side (instance-owned) |
|---|---|---|
| Extra devcontainer tooling | `.devcontainer/Dockerfile` `develop` stage runs every `scripts/develop.d/*.sh` after `develop.sh` | `scripts/develop.d/NN-name.sh` |
| Extra post-create steps | `scripts/post-create.sh`, run by `devcontainer.json`'s `postCreateCommand`, runs every `scripts/post-create.d/*.sh` | `scripts/post-create.d/NN-name.sh` |
| Extra compose services / dev-service settings | `devcontainer.json` `"dockerComposeFile": ["compose.yml", "compose.instance.yml"]` | `.devcontainer/compose.instance.yml` (base ships a stub, `ignore` tier) |
| Runtime image | nothing (the devcontainer image lives in `.devcontainer/`) | top-level `Dockerfile` + `.dockerignore`, listed in `.github/template-sync-manifest.local.yml`'s `ignore:` |

Files with no include mechanism of their own (JSON configs,
`.pre-commit-config.yaml` — prek has no include) are `merge` tier, so an
instance's additions survive a sync. The runtime image is the
top-level `Dockerfile` with a plain `.dockerignore`, both instance-owned:
base keeps the devcontainer image and its excludes under
`.devcontainer/`, where BuildKit picks up
`.devcontainer/Dockerfile.dockerignore` for that build automatically.

**One-time upgrade (instances synced from before this layout).** A sync
adds `.devcontainer/Dockerfile` but can't delete the old top-level
`Dockerfile`/`.dockerignore`: from this release on, base's manifest no
longer classifies those paths, and a merge-tier file is never deleted
automatically. In the sync PR, by hand:

- With a runtime image: rename `app.Dockerfile` → `Dockerfile` and
  `app.Dockerfile.dockerignore` → `.dockerignore`, overwriting the old
  base files. Update every reference (`-f app.Dockerfile` in the
  `Makefile`, `dockerfile:` in compose files, CI, docs) and list both
  paths under `ignore:` in `.github/template-sync-manifest.local.yml`.
- Without one: delete the old top-level `Dockerfile` and `.dockerignore`.

## Getting started

1. Open this folder in a devcontainer (VS Code: "Reopen in Container" —
   `.vscode/extensions.json` recommends the extension that offers this —
   or any tool that reads `.devcontainer/devcontainer.json`). This starts
   the app alongside Postgres, Redis, S3 (RustFS), Keycloak (OIDC), MQTT,
   and a Selenium container Playwright can drive remotely; builds the
   `develop` stage; and installs the git hooks and warms the cargo build,
   all via `postCreateCommand` (`scripts/post-create.sh`).
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

Without a devcontainer: install the pinned Rust toolchain
(`scripts/develop.d/10-rust.sh`'s `RUST_VERSION`) via
[`rustup`](https://rustup.rs/), export the
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
the `devcontainer-lock` lockfile-consistency check, the image-layer hooks
(`hadolint`, `check-dockerfile-versions`, `image-scan`; see "Image
layer"), `check-rust-version` (the Rust pin in `Dockerfile`,
`scripts/develop.d/10-rust.sh` and `Cargo.toml` agree), and this
instance's own Rust hooks: `cargo fmt --check` and `cargo clippy
--all-targets -- -D warnings` (both on every commit); `cargo check
--all-targets`, `cargo test` (unit tier colocated in `src/`, plus the
integration/e2e-equivalent tiers in `tests/` — see `tests/README.md`),
`cargo llvm-cov` (a 99% line-coverage floor — see
`docs/adrs/0010-80-percent-line-coverage-floor-via-cargo-llvm-cov.md` and
`docs/nfrs/NFR-0023-test-coverage-gate.md`), and `cargo audit`
(dependency vulnerabilities) on push only, since they need the
devcontainer stack's real backing services and take longer to run.

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
— as an `amd64`/`arm64` matrix, both legs native (no QEMU). `perf.yml`
runs the `tests/perf` `goose` load test against the built `runner` image
on `workflow_dispatch` (not per-PR — see
`docs/adrs/0018-goose-for-load-testing-not-locust.md`).
`.github/workflows/release.yml` is triggered manually to cut an
alpha/beta/rc/full release — see "Release: a Makefile contract" below and
`.github/workflows/README.md`.

## Release: a Makefile contract

`release.yml` computes the next SemVer tag (`compute_next_version.py`,
unchanged across every instance), then drives these targets:

| Target | Runs | Where | Env available |
|---|---|---|---|
| `release-arches` | once, first | host runner | none; must print a JSON array on stdout, e.g. `["amd64"]` |
| `build`, `sbom`, `release-assets`, `publish` | once **per arch**, in that order | host runner for that arch | `RELEASE_VERSION`, `RELEASE_TAG`, `RELEASE_ARCH`, `OCI_REGISTRY`, `OCI_IMAGE_NAME`, `OCI_REGISTRY_USERNAME`, `OCI_REGISTRY_PASSWORD`, `PYPI_PUBLISH`, `NPM_PUBLISH`, `CARGO_PUBLISH`, all `GITHUB_*`; on `publish` only, `NPM_TOKEN` and `CARGO_REGISTRY_TOKEN` (bootstrap secrets; empty when unset, so treat empty as absent) |
| `release-check` | once, after all arches | **inside the devcontainer** (`devcontainers/ci`), stack up | `RELEASE_VERSION`, `RELEASE_TAG`, `PYPI_PUBLISH`, `NPM_PUBLISH`, `CARGO_PUBLISH` |
| `finalize` | once, after `release-check` | host runner | same as per-arch minus `RELEASE_ARCH` |

What each target is for: `build` produces the release artifact(s) (an
image, `cargo build --release`, `npm pack`, `helm package`, ...); `sbom`
writes an SBOM for what `build` produced; `release-assets` populates
`dist/` (gitignored) with every file to attach to the GitHub release;
`publish` pushes to whatever registry applies, skipping cleanly when its
variable/secret isn't set; `release-check` runs checks that need the
backing services (e.g. tests producing a coverage report); `finalize`
does cross-arch work such as combining per-arch registry tags into one
multi-arch manifest list. `RELEASE_VERSION` (e.g. `1.2.3` or
`1.2.3-alpha.1`) and `RELEASE_TAG` (the same, `v`-prefixed) are for
embedding or tagging with the version. A single-arch no-op is the
default: `release-arches` prints `["amd64"]`.

`dist/` rules: each arch leg's `dist/` is uploaded as artifact
`release-assets-<arch>`, and the final job downloads all of them into one
`dist/`, so **per-arch file names must include the arch**. `release-check`
and `finalize` may add more files to `dist/`. `release.yml` then runs
`gh release create` with every file left in `dist/`.

The arch legs run natively, not under QEMU: `arm64` on `ubuntu-24.04-arm`,
`amd64` on `ubuntu-24.04`, each overridable with the `CI_RUNNER_ARM64` /
`CI_RUNNER_AMD64` repository/organization variables.

`template-docker` replaces that no-op `Makefile` with the image
implementation (see "Image layer (template-docker)" below). The base
`Makefile` implements every target as a documented
no-op, since `template-base` ships no artifact — it's both a working
example of the contract's shape and what `release.yml` needs to run
cleanly if this repo itself is ever released. An instance's `Makefile` is
not template-owned (it's the whole point that it differs per artifact
type), so it's tracked like any other instance-owned file — untouched by
this template's own sync manifest.

This instance's `image.mk` adds two `release-check` steps: `coverage`
(`dist/coverage.xml`, the asset the image's coverage-report label points
at) and `stack-smoke` (`NFR-0010`: the built image against the real
Postgres/Redis/S3/Keycloak stack, polling `/health/ready`). The per-arch
smoke before each push runs the image under `MODE=mock` and polls
`/health/live`. Both run before `finalize`, so a release whose image
can't report healthy gets no manifest list, tag or GitHub release.

## Image layer (template-docker)

`template-docker` is a mid-layer template: an instance of `template-base`
that is itself the template for every app that ships a container image
(`template-fastapi`, `template-axum`, `template-react`). Sync runs in
three hops, base to docker to app, and `ADR 0001` records why. It adds
these files over base:

| Path | Tier for instances | What it is |
|---|---|---|
| `Makefile` | `replace` | the image implementation of the release contract above |
| `image.mk` | `ignore` | the instance's settings for the Makefile, every variable commented |
| `Dockerfile` | `ignore` | skeleton to replace: `builder` and `runner` stages, `ARG DEBIAN_VERSION`, `ARG APP_UID`, SSL ARGs, `APP_VERSION`/`APP_COMMIT` |
| `.dockerignore` | `merge` | baseline build-context excludes for the runtime image (including `node_modules`, `coverage`, `target` and `dist`); the instance appends its own |
| `scripts/image-smoke.sh` | `replace` | loads the image tar from `dist/`, runs it and polls a URL |
| `scripts/runner-setup.sh` | `replace` | optional: creates the group-0 runtime user and its writable directories for the `runner` stage |
| `scripts/image-scan.sh` | `replace` | the Trivy scan the `image-scan` hook runs |
| `scripts/test-image-scripts.sh` | `replace` | offline tests of the Makefile and `scripts/` logic, run by the `image-scripts-test` hook |
| `scripts/runner-setup.d/*.sh` | `replace` for the scripts template-axum ships (`10-ca-certificates.sh`); an instance's own go in `.github/template-sync-manifest.local.yml` | runner-stage hooks |
| `scripts/check-dockerfile-versions.sh` | `replace` | the version-ARG consistency check |

### `image.mk` variables

The Makefile starts with `-include image.mk` (a missing file means all
defaults) and every variable is `?=`, so set them in `image.mk`:

| Variable | Default | Meaning |
|---|---|---|
| `DOCKERFILE`, `TARGET` | `Dockerfile`, `runner` | build file and stage |
| `ARCHES` | `amd64 arm64` | space-separated; `release-arches` prints it as a JSON array and `finalize` merges the per-arch images into one tag; must not be empty |
| `BUILD_ARGS` | empty | full flag words, e.g. `--build-arg APP_VERSION=$(RELEASE_VERSION)` |
| `EXTRA_LABELS` | empty | full flag words, e.g. `--label k=v` |
| `OCI_DESCRIPTION` | empty | `org.opencontainers.image.description`; the label is omitted while empty |
| `OCI_LICENSE` | `GPL-3.0-or-later` | SPDX id for `org.opencontainers.image.licenses` |
| `COVERAGE_REPORT` | empty | release asset name for the `io.github.<owner>.coverage-report` label; omitted while empty |
| `IMAGE_SMOKE_PORT`, `IMAGE_SMOKE_PATH` | `8000`, `/health/live` | URL polled for a 2xx |
| `IMAGE_SMOKE_RUN_ARGS`, `IMAGE_SMOKE_TIMEOUT` | empty, `60` | extra `docker run` words, whitespace-separated with no embedded spaces or quotes (passed through the environment); seconds to wait |

`SYFT_VERSION` (the SBOM tool image tag) and `SKOPEO_IMAGE` (tag and
digest) are pinned in the Makefile for Renovate; don't override them. The
image name comes from `GITHUB_REPOSITORY` (the repository name), never a
hardcoded one. Inside the devcontainer, where `release-check` runs and
`GITHUB_REPOSITORY` is unset, it is derived from the `origin` remote
(`ssh://[user@]host[:PORT]/owner/repo`, scp-like or https form; tested by
`scripts/test-image-scripts.sh`). If git fails (no `origin`, "dubious
ownership"), there is no guess: `build`, `sbom`, `release-assets`, `publish`
and `finalize` print git's error to stderr and fail, and
`GITHUB_REPOSITORY=owner/repo` overrides;
`image-smoke` also accepts the single `dist/*-<version>-<arch>.tar` when
the expected name is absent. `OCI_IMAGE_NAME` overrides the registry path.
`build`, `sbom`, `release-assets` and `publish` need `RELEASE_ARCH` and
fail through the `check-arch` prerequisite when it is empty;
`release-arches`, `release-check`/`image-smoke` (host arch when unset) and
`finalize` don't. `publish` copies the tar from `dist/` that was SBOM'd
and smoke-tested to the registry with skopeo (`copy --all`, `oci-archive:`
to `docker://`, Apache-2.0, pinned image; `crane push` only reads
docker-format tarballs, and `build` writes an OCI layout), so the pushed
digest is the tested one. `--all` pushes the tar's whole index as
`<version>-<arch>`: the image for `RELEASE_ARCH` plus the buildx attestation
manifests (provenance, SBOM) when the build wrote any. Without it skopeo
picks one image by the *host's* platform, which can fail or pick the wrong
image when `RELEASE_ARCH` isn't native, and it drops the attestations. It
records that index's digest in `dist/<name>-<version>-<arch>.digest` (which
`finalize` deletes after use, so release.yml never attaches digest files to
the GitHub release), and
`finalize` builds the manifest list from those digests (`imagetools create`
merges the per-arch indexes, so the attestations carry over). `finalize`
logs in with a throwaway `DOCKER_CONFIG`, into which it links the user's
`cli-plugins` directory (Docker looks up plugins in
`$DOCKER_CONFIG/cli-plugins`), so a user-installed buildx is still found. Registry login uses a throwaway authfile fed the
password on stdin. The labels are listed in `.github/workflows/README.md`'s "Image
labels". Registry auth stays the opt-in `OCI_REGISTRY*` secrets contract.

### Adding `release-check` steps

`release-check` has no recipe of its own: it depends on `image-smoke`.
An instance adds steps as prerequisites in `image.mk`, so Make never
sees two recipes for one target:

```make
release-check: instance-tests
instance-tests:
	uv run pytest --cov-report=xml:dist/coverage.xml
```

`release-assets` depends on `image-smoke`, so each arch is booted
natively on its own runner in the build job before `publish`; the
`release-check` run inside the devcontainer smoke-tests the host arch
again with the backing services up. `image-smoke` converts the arch's
image in the OCI-layout tar with the pinned skopeo (through
`scripts/skopeo.sh`, which `publish` uses too; `--override-arch`, to a
docker-archive under `dist/`) and `docker load`s that, because
`docker load` can't read a buildx OCI tar on the classic image store,
with or without attestations. It reads `IMAGE_TAR_PREFIX`,
`SKOPEO_IMAGE` and `ARCHES` (set by the Makefile), the arch from
`RELEASE_ARCH` (else the host's). A missing tar for that arch fails, so a
run never passes without having tested an image; the one exception is
`RELEASE_ARCH` unset with a host arch outside `ARCHES` (the instance
doesn't build it, and its own build leg tested each arch it does). It maps
a random host port to `IMAGE_SMOKE_PORT`, prints the container logs on failure and always
removes the container and the private `image-smoke-*` tag it created
(never an image that existed before).

### Runner setup hook

Optional. It serves a runtime image that needs the group-0 user and
writable directories; an instance whose `runner` stage doesn't call it
(e.g. `template-react`'s nginx runner, whose base image already has its own
non-root user) simply never runs it. Both files stay in the tree, since
they are template-owned, and nothing checks or requires them.

`scripts/runner-setup.sh` creates the user (UID `APP_UID`, group 0), makes
its home and `/app/data` group-writable, and leaves `/app` root-owned, so
the image also runs under an arbitrary UID (OpenShift's restricted SCC).
Run it **before** copying app code, then copy the code after it with
`COPY --chown=<uid>:0` of content whose modes are already 0550
directories and 0440 files: only data and cache directories are writable, the runtime user
can't modify what it runs, and no recursive `chgrp`/`chmod` duplicates
the files into another layer. Set those modes numerically, per kind
(`find . -type d -exec chmod 0550 {} +`, `find . -type f -exec chmod 0440
{} +`, e.g. in the builder stage, or `COPY --chmod=0550`/`0440` for sources
that are all one kind), not with the symbolic `--chmod=u=rX,g=rX,o=`: that
form left files unreadable to the app user on GitHub's stock-runner Docker
(template-fastapi's CI smoke, even with `# syntax=docker/dockerfile:1.28`),
though it worked on a current local daemon. It then runs every
`scripts/runner-setup.d/*.sh` in glob order as `bash NN-name.sh APP_UID`,
still as root and in the same `RUN` layer. The instance puts extra
packages and writable directories there instead of editing the script.

### Dockerfile version check

The `check-dockerfile-versions` hook runs
`scripts/check-dockerfile-versions.sh DEBIAN_VERSION` whenever either
Dockerfile, the script or `.pre-commit-config.yaml` (where the ARG list
lives) changes: each named ARG must have the same default in
`.devcontainer/Dockerfile` and `Dockerfile` (the runtime must match the
toolchain the devcontainer tests against). An ARG may be declared several
times in one file (a bare `ARG NAME` re-declares it inside a stage); bare
ones are ignored and every default given must agree. An instance appends the ARG
names it keeps in both files (e.g. `RUST_VERSION`) to the hook's `entry:`
in its `.pre-commit-config.yaml`; ARGs not listed aren't checked.

### Image hooks

- `hadolint` lints both Dockerfiles, run from the pinned official image
  (so it needs Docker). It fails on warnings and above, not on info-level
  findings, and has no repo-wide ignores. An instance that must
  install an unpinned apt package adds a per-line
  `# hadolint ignore=DL3008` with a reason (as the skeleton `Dockerfile`
  does: Debian drops superseded versions, so pins break rebuilds).
- `image-scripts-test` runs `scripts/test-image-scripts.sh`: offline tests
  of the Makefile's repo derivation, the version check, the smoke test's
  failure on a missing tar and `skopeo.sh`'s argument handling. It needs no
  Docker.
- `image-scan` runs only at the `manual` stage, because it needs
  the tar from `make build` and network access for the vulnerability
  database. It scans only `dist/*-<RELEASE_VERSION>-<arch>.tar` (arch from
  `RELEASE_ARCH`, else the host's; without `RELEASE_VERSION`, the newest
  `dist/*-<arch>.tar`), never stale tars of other versions. With no such tar
  it skips. Otherwise `scripts/image-scan.sh` runs a
  pinned Trivy image on that tar and fails on HIGH or CRITICAL
  findings that have a fix (`--severity HIGH,CRITICAL --ignore-unfixed
  --exit-code 1`). Trivy can't read the OCI-layout tar `make build`
  writes, so the script unpacks it and scans the directory, selecting
  `linux/<arch>` from the tar's name so a buildx attestation manifest is
  never scanned instead of the image. The database is cached in the `trivy-cache` Docker
  volume. Trivy is Apache-2.0, so invoking it as a tool is compatible
  with this repo's GPL-3.0 licence. The scanner is the maintainers'
  choice, made over Grype. Trivy's releases and Docker Hub images were
  compromised in March 2026 (GHSA-69fq-xp46-6x23), so the image is pinned
  by digest as well as tag (`aquasec/trivy:<version>@sha256:…`); the
  advisory lists images referenced by digest as unaffected. Renovate
  updates tag and digest together. When bumping by hand, take the digest
  from the registry and verify it with cosign as the advisory describes:
  `cosign verify --certificate-identity-regexp
  'https://github\.com/aquasecurity/' --certificate-oidc-issuer
  'https://token.actions.githubusercontent.com'
  ghcr.io/aquasecurity/trivy@sha256:…` (the Docker Hub, GHCR and ECR
  digests of a release are identical).

### Re-parenting an existing app onto template-docker

One-time steps for an app that is today an instance of `template-base`
(or of a sibling template) and moves under `template-docker`:

1. Make the tracked files equal `template-docker`'s tag: take the
   template's `Makefile`, `scripts/` files, `.dockerignore` and
   `.pre-commit-config.yaml` hooks, move the app's Makefile settings into
   `image.mk` and its runtime user setup into `scripts/runner-setup.d/`,
   and list its version ARGs on the `check-dockerfile-versions` hook.
2. For `template-react` and `template-axum`, which were not synced from
   base, migrate to base's layout first (devcontainer, workflows, docs,
   manifest) and let a base sync PR settle before this step.
3. Bootstrap the sync state: run `template-sync.yml` manually with
   `template_repo=craftainer/template-docker` and `initial_sync_tag` set
   to the tag the files now equal. That writes
   `.github/template-sync-state.json` without changing files. From then on
   syncs track `template-docker`, which in turn tracks base.
4. Resolve base's additions to the manifest by hand when
   `template-docker` is the one syncing from base, as the header of
   `.github/template-sync-manifest.yml` says.

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

An instance that isn't itself a template shouldn't edit that manifest to
list its own files (`src/`, say): it's `merge`-tier, so edits would
conflict with later syncs. It lists them under an `ignore:` key in the
instance-owned
`.github/template-sync-manifest.local.yml` instead; the
`template-sync-manifest` check merges that file in, but sync never reads
it, and it accepts no other tier.

`template-axum` is itself both an instance of `template-docker` and a
template for further instances: its own
`.github/template-sync-manifest.yml` classifies its *own* tracked files
for *its* downstream instances, entirely separate from template-docker's
manifest (it edits the manifest directly and resolves template-docker's
additions by hand). Sync runs in three hops: base to docker to axum to the
instance.

## Versions and config

Every version and config value is defined in exactly one place; nothing
duplicates or re-pins it elsewhere. Everything pinned here (base image,
Actions, hook revisions, `prek`/Claude Code CLI/`snip`/`gh`/`uv`/Python/
Node.js/`cargo-audit`/`cargo-llvm-cov` versions) is pinned once, at its single point of use, to an exact
patch version — never a floating range or `latest` — so Renovate can
bump them one at a time and the diff shows exactly what changed. An
instance's own language runtime/package versions follow the same rule
in its own Dockerfile/manifest. Specifically:

- Rust toolchain version: `RUST_VERSION` in the `Dockerfile` (the
  `builder` stage pulls it as the official `rust` image tag) and in
  `scripts/develop.d/10-rust.sh` (the devcontainer installs it via
  `rustup`). `.devcontainer/Dockerfile` is template-owned and can't carry
  the ARG, so `scripts/check-rust-version.sh` keeps the two equal, and
  `Cargo.toml`'s `rust-version` at their major.minor.
- Rust crate versions: `Cargo.toml` / `Cargo.lock`, pinned to exact
  versions — use `cargo add <crate>` / `cargo update` rather than editing
  the dependency lists by hand.

`cargo-llvm-cov`'s coverage data and `target/` both live outside the
bind-mounted workspace (`CARGO_TARGET_DIR`, set in
`.devcontainer/compose.instance.yml`): a build directory inside the
bind-mounted `/workspace` gets scanned file-by-file by host
antivirus/malware tools on Windows and is painfully slow to write to. A
new tool with its own on-disk cache follows the same pattern.

`python3` and Node.js, both installed by `scripts/develop.sh` and pinned
via `.devcontainer/Dockerfile`'s `PYTHON_VERSION`/`NODE_VERSION` ARGs, are
infrastructure tooling only — not an application runtime this template
assumes. `python3` exists because `prek` needs an interpreter to build
the venv for any `language: python` hook (`.pre-commit-config.yaml`'s
`pre-commit-hooks` repo) and because `.github/scripts/*.py` need one
directly; `uv` (its own `UV_VERSION` ARG) is only the mechanism used to
install that exact, checksum-verified CPython build rather than an
unpinned apt package, and stays on `PATH` afterward. Node.js exists
because `npx` (the `clear-thought` and `playwright` MCP servers in `.mcp.json`) needs it.
An instance that adds Python or Node.js as its own application runtime
can reuse these directly rather than installing a second copy.

## Code style

Every file gets a brief header stating what the file *is for* — one
line, sometimes two: a leading comment for most formats, a module `//!`
doc comment for Rust modules. Never describe the file's contents
in the header; that's what reading the file is for. Markdown files' own
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
  `.devcontainer/`.
- Assume more than this template defines — a fresh instance carries only
  what this template itself ships.
