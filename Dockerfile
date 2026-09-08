# syntax=docker/dockerfile:1.27
# Three-stage build for the app: develop (devcontainer), builder, runner.

ARG DEBIAN_VERSION=trixie

# The Rust toolchain: builder/runner pull this exact patch version as the
# official `rust`/`debian` image tags (see those stages below); develop
# installs the same version itself via rustup (see RUSTUP_VERSION), so all
# three stages compile with an identical compiler -- mirrors template-fastapi's
# own PYTHON_VERSION pattern.
# renovate: datasource=docker depName=rust
ARG RUST_VERSION=1.91.1

# renovate: datasource=github-releases depName=rust-lang/rustup
ARG RUSTUP_VERSION=1.28.2

# renovate: datasource=crate depName=cargo-audit
ARG CARGO_AUDIT_VERSION=0.22.2

# renovate: datasource=crate depName=cargo-cyclonedx
ARG CARGO_CYCLONEDX_VERSION=0.5.7

# renovate: datasource=crate depName=cargo-llvm-cov
ARG CARGO_LLVM_COV_VERSION=0.9.1

# renovate: datasource=github-releases depName=j178/prek
ARG PREK_VERSION=0.5.2

# renovate: datasource=npm depName=@anthropic-ai/claude-code
ARG CLAUDE_CODE_VERSION=2.1.263

# renovate: datasource=github-releases depName=edouard-claude/snip
ARG SNIP_VERSION=0.25.1

# renovate: datasource=github-releases depName=rustfs/cli
ARG RUSTFS_CLI_VERSION=0.1.32

# uv/python3/Node.js are infrastructure tooling only, not this app's
# runtime -- see scripts/README.md and develop.sh's comments. uv installs
# the pinned CPython build prek needs for any "language: python" hook and
# .github/scripts/*.py; Node.js's npx is only for the clear-thought MCP
# server in .mcp.json.
# renovate: datasource=github-releases depName=astral-sh/uv
ARG UV_VERSION=0.12.10
# renovate: datasource=python-version depName=python
ARG PYTHON_VERSION=3.14.7
# renovate: datasource=node-version depName=node
ARG NODE_VERSION=24.20.0

ARG APP_UID=1000

ARG SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
ARG SSL_CERT_DIR=/etc/ssl/certs

########################################
# develop — interactive devcontainer image, based on Microsoft's generic
# base devcontainer image, with rustup installing the pinned Rust toolchain
# itself (see RUST_VERSION above). Source is bind-mounted, not copied.
########################################
FROM mcr.microsoft.com/devcontainers/base:${DEBIAN_VERSION} AS develop
ARG RUST_VERSION
ARG RUSTUP_VERSION
ARG CARGO_AUDIT_VERSION
ARG CARGO_CYCLONEDX_VERSION
ARG CARGO_LLVM_COV_VERSION
ARG PREK_VERSION
ARG CLAUDE_CODE_VERSION
ARG SNIP_VERSION
ARG RUSTFS_CLI_VERSION
ARG UV_VERSION
ARG PYTHON_VERSION
ARG NODE_VERSION
ARG SSL_CERT_FILE
ARG SSL_CERT_DIR

# Keep cargo's registry/target dirs outside the bind-mounted /workspace: on
# Windows hosts a target/ inside the mount gets scanned file-by-file by
# antivirus/malware tools and is painfully slow to compile into -- same
# reasoning as UV_PROJECT_ENVIRONMENT in template-fastapi's own Dockerfile.
ENV CARGO_HOME=/home/vscode/.cargo \
    RUSTUP_HOME=/home/vscode/.rustup \
    CARGO_TARGET_DIR=/home/vscode/.cache/cargo-target \
    SSL_CERT_FILE=${SSL_CERT_FILE} \
    SSL_CERT_DIR=${SSL_CERT_DIR} \
    REQUESTS_CA_BUNDLE=${SSL_CERT_FILE} \
    CURL_CA_BUNDLE=${SSL_CERT_FILE} \
    PATH=/home/vscode/.cargo/bin:${PATH}

COPY scripts/develop.sh /tmp/develop.sh
RUN bash /tmp/develop.sh "$RUST_VERSION" "$RUSTUP_VERSION" "$CARGO_AUDIT_VERSION" "$CARGO_CYCLONEDX_VERSION" "$CARGO_LLVM_COV_VERSION" "$PREK_VERSION" "$CLAUDE_CODE_VERSION" "$SNIP_VERSION" "$RUSTFS_CLI_VERSION" "$UV_VERSION" "$PYTHON_VERSION" "$NODE_VERSION"

USER vscode
WORKDIR /workspace
CMD ["sleep", "infinity"]

########################################
# builder — compiles a release binary. Dependencies are built from a dummy
# main.rs before the real source is copied in (builder-sync-deps.sh /
# builder-sync-app.sh), so a source-only change doesn't invalidate the
# (much slower) dependency-compilation layer -- the same shape as
# template-fastapi's uv-based builder-sync-deps.sh/builder-sync-app.sh
# split. Chosen over cargo-chef to avoid a third "planner" stage; revisit if
# this project's dependency graph grows enough that chef's finer-grained
# recipe caching starts paying for that extra stage.
########################################
FROM rust:${RUST_VERSION}-slim-${DEBIAN_VERSION} AS builder
ARG SSL_CERT_FILE
ARG SSL_CERT_DIR

ENV SSL_CERT_FILE=${SSL_CERT_FILE} \
    SSL_CERT_DIR=${SSL_CERT_DIR} \
    REQUESTS_CA_BUNDLE=${SSL_CERT_FILE} \
    CURL_CA_BUNDLE=${SSL_CERT_FILE}

COPY scripts/builder.sh /tmp/builder.sh
RUN bash /tmp/builder.sh

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY scripts/builder-sync-deps.sh /tmp/builder-sync-deps.sh
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    bash /tmp/builder-sync-deps.sh

COPY src ./src
COPY scripts/builder-sync-app.sh /tmp/builder-sync-app.sh
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    bash /tmp/builder-sync-app.sh

########################################
# runner — minimal runtime image: debian-slim (not distroless) because
# scripts/runner-setup.sh's useradd/chgrp/chmod need a shell and coreutils
# that distroless intentionally omits; the base already has no package
# manager cruft beyond that. Runs under an arbitrary UID (e.g. OpenShift's
# restricted SCC) as well as a fixed one — see runner-setup.sh and the
# --chown/--chmod below.
########################################
FROM debian:${DEBIAN_VERSION}-slim AS runner
ARG APP_UID
ARG SSL_CERT_FILE
ARG SSL_CERT_DIR

ENV HOME=/home/appuser \
    SSL_CERT_FILE=${SSL_CERT_FILE} \
    SSL_CERT_DIR=${SSL_CERT_DIR} \
    REQUESTS_CA_BUNDLE=${SSL_CERT_FILE} \
    CURL_CA_BUNDLE=${SSL_CERT_FILE}

COPY scripts/runner.sh /usr/local/bin/runner.sh
# Group 0, not the numeric owner, is what an arbitrary UID actually gets
# under OpenShift's restricted SCC — see runner-setup.sh's comment. Set
# here rather than with a recursive RUN chmod, which would be slow over a
# whole binary/cert tree.
COPY --from=builder --chown=${APP_UID}:0 --chmod=750 /build/bin/template-axum /app/template-axum
WORKDIR /app

COPY scripts/runner-setup.sh /tmp/runner-setup.sh
RUN bash /tmp/runner-setup.sh "$APP_UID"

USER $APP_UID
EXPOSE 8000
ENTRYPOINT ["runner.sh"]
