# syntax=docker/dockerfile:1.28
# Two-stage runtime image for the app: builder -> runner. The devcontainer
# image is the base-owned .devcontainer/Dockerfile's `develop` stage (Rust
# comes from scripts/develop.d/10-rust.sh); this file is the repo's only
# top-level Dockerfile and `make build` (see image.mk) builds its `runner`
# stage.

# Also declared in .devcontainer/Dockerfile; scripts/check-dockerfile-versions.sh
# keeps the two equal.
ARG DEBIAN_VERSION=trixie

# The Rust toolchain of the builder stage, as the official `rust` image tag.
# The devcontainer installs the same version in scripts/develop.d/10-rust.sh
# (kept equal by scripts/check-rust-version.sh), so tests and release builds
# compile with an identical compiler.
# renovate: datasource=docker depName=rust
ARG RUST_VERSION=1.99.1

ARG APP_UID=1000

ARG SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
ARG SSL_CERT_DIR=/etc/ssl/certs

########################################
# builder — compiles a release binary. Dependencies are built from a dummy
# main.rs before the real source is copied in (builder-sync-deps.sh /
# builder-sync-app.sh), so a source-only change doesn't invalidate the
# (much slower) dependency-compilation layer. Chosen over cargo-chef to
# avoid a third "planner" stage; revisit if this project's dependency graph
# grows enough that chef's finer-grained recipe caching starts paying for
# that extra stage.
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
# tests/perf is a workspace member (see Cargo.toml): cargo must be able to
# load its manifest, though only "." is built (builder-sync-deps.sh adds a
# stub target for it).
COPY tests/perf/Cargo.toml ./tests/perf/Cargo.toml
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
# that distroless intentionally omits. Runs under an arbitrary UID (e.g.
# OpenShift's restricted SCC) as well as a fixed one: see runner-setup.sh.
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

# Create the user and the few writable directories first (home, /app/data),
# then copy the binary read-only on top: no recursive chgrp/chmod layer, and
# the runtime user can't modify what it runs.
COPY scripts/runner-setup.sh /tmp/runner-setup.sh
COPY scripts/runner-setup.d/ /tmp/runner-setup.d/
RUN bash /tmp/runner-setup.sh "$APP_UID" && rm -rf /tmp/runner-setup.sh /tmp/runner-setup.d

COPY --chmod=755 scripts/runner.sh /usr/local/bin/runner.sh
WORKDIR /app
COPY --from=builder --chown=${APP_UID}:0 --chmod=u=rx,g=rx,o= /build/bin/template-axum /app/template-axum

# Set by `make build` through BUILD_ARGS in image.mk; declared last so a new
# commit hash doesn't invalidate the layers above.
ARG APP_VERSION=
ARG APP_COMMIT=
ENV APP_VERSION=${APP_VERSION} \
    APP_COMMIT=${APP_COMMIT}

USER $APP_UID
EXPOSE 8000
ENTRYPOINT ["runner.sh"]
