#!/usr/bin/env bash
# Installs the Rust toolchain (rustup, pinned) and the cargo tools the
# pre-commit hooks call (cargo-audit, cargo-llvm-cov), plus the system
# libraries some -sys crates link against. Runs as root during the
# `develop` stage build (see README.md here).
#
# Keep RUST_VERSION in step with the top-level Dockerfile's RUST_VERSION ARG
# (builder stage) and Cargo.toml's `rust-version`;
# scripts/check-rust-version.sh enforces the first.
set -euo pipefail

# renovate: datasource=docker depName=rust
RUST_VERSION=1.99.1
# renovate: datasource=github-releases depName=rust-lang/rustup
RUSTUP_VERSION=1.28.2
# renovate: datasource=crate depName=cargo-audit
CARGO_AUDIT_VERSION=0.22.2
# renovate: datasource=crate depName=cargo-llvm-cov
CARGO_LLVM_COV_VERSION=0.9.1

# pkg-config/libssl-dev: some cargo dependencies (e.g. aws-sdk-s3)
# link against OpenSSL via -sys crates even though the app's own TLS is
# rustls. libpq-dev: the Postgres client library for the same reason.
apt-get update
apt-get install -y --no-install-recommends libpq-dev pkg-config libssl-dev
apt-get clean
rm -rf /var/lib/apt/lists/*

# rustup-init, fetched as a release binary for the pinned RUSTUP_VERSION and
# checksum-verified against static.rust-lang.org's own published .sha256,
# rather than trusting the unversioned https://sh.rustup.rs installer.
rustup_arch="$(dpkg --print-architecture)"
case "$rustup_arch" in
    amd64) rustup_target=x86_64-unknown-linux-gnu ;;
    arm64) rustup_target=aarch64-unknown-linux-gnu ;;
    *)
        echo "unsupported architecture for rustup install: $rustup_arch" >&2
        exit 1
        ;;
esac
rustup_tmpdir="$(mktemp -d)"
chmod a+rwx "$rustup_tmpdir"
rustup_url="https://static.rust-lang.org/rustup/archive/${RUSTUP_VERSION}/${rustup_target}/rustup-init"
curl -LsSf -o "${rustup_tmpdir}/rustup-init" "$rustup_url"
curl -LsSf -o "${rustup_tmpdir}/rustup-init.sha256" "${rustup_url}.sha256"
(cd "$rustup_tmpdir" && sha256sum -c rustup-init.sha256)
chmod +x "${rustup_tmpdir}/rustup-init"
# llvm-tools-preview is what cargo-llvm-cov instruments with.
sudo -u vscode env HOME=/home/vscode "${rustup_tmpdir}/rustup-init" -y \
    --default-toolchain "$RUST_VERSION" --profile default \
    -c llvm-tools-preview --no-modify-path
rm -rf "$rustup_tmpdir"
for bin in rustup cargo rustc rustfmt cargo-clippy clippy-driver rustdoc; do
    ln -sf "/home/vscode/.cargo/bin/${bin}" "/usr/local/bin/${bin}"
done

# For the pre-commit/CI checks (`cargo audit`, the cargo-llvm-cov coverage
# gate -- tests/README.md's "Coverage gate"). cargo-installed and pinned.
# CARGO_TARGET_DIR keeps the build artefacts out of the image's home tree.
for crate in "cargo-audit@${CARGO_AUDIT_VERSION}" "cargo-llvm-cov@${CARGO_LLVM_COV_VERSION}"; do
    sudo -u vscode env HOME=/home/vscode CARGO_TARGET_DIR=/tmp/cargo-install-target \
        /home/vscode/.cargo/bin/cargo install --locked "$crate"
done
rm -rf /tmp/cargo-install-target
ln -sf /home/vscode/.cargo/bin/cargo-audit /usr/local/bin/cargo-audit
ln -sf /home/vscode/.cargo/bin/cargo-llvm-cov /usr/local/bin/cargo-llvm-cov
