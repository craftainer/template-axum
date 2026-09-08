#!/usr/bin/env bash
# Sets up the `develop` stage on top of Microsoft's generic base
# devcontainer image (which already provides the `vscode` user, git, sudo,
# and curl); Rust itself is installed below via rustup, pinned to the same
# exact version as the builder/runner stages' own `rust`/`debian` image tags.
set -euo pipefail

rust_version=$1
rustup_version=$2
cargo_audit_version=$3
cargo_cyclonedx_version=$4
cargo_llvm_cov_version=$5
prek_version=$6
claude_code_version=$7
snip_version=$8
rustfs_cli_version=$9
uv_version=${10}
python_version=${11}
node_version=${12}

# postgresql-client/redis-tools give psql/redis-cli for connecting to the
# stack's postgres/redis services (see .devcontainer/stack/postgres and
# .devcontainer/stack/redis's READMEs). Debian's own repo only carries one
# version of each -- no exact-version pin available via apt, same reasoning
# as template-fastapi's own develop.sh; a newer client talking to an older
# server is standard practice for both. pkg-config/libssl-dev: some cargo
# dependencies (phase 2's aws-sdk-s3/openidconnect stack) link against
# OpenSSL via -sys crates even though the app's own TLS is rustls. xz-utils:
# Node's own release tarballs are .tar.xz.
apt-get update
apt-get install -y --no-install-recommends \
    libpq-dev postgresql-client redis-tools pkg-config libssl-dev xz-utils
apt-get clean
rm -rf /var/lib/apt/lists/*

# rustup-init, fetched as a release binary for the pinned RUSTUP_VERSION and
# checksum-verified against static.rust-lang.org's own published .sha256 --
# same checksummed-release pattern as snip/rustfs-cli below, rather than
# trusting the unversioned https://sh.rustup.rs installer.
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
sudo chmod a+rwx "$rustup_tmpdir"
rustup_url="https://static.rust-lang.org/rustup/archive/${rustup_version}/${rustup_target}/rustup-init"
curl -LsSf -o "${rustup_tmpdir}/rustup-init" "$rustup_url"
curl -LsSf -o "${rustup_tmpdir}/rustup-init.sha256" "${rustup_url}.sha256"
(cd "$rustup_tmpdir" && sha256sum -c rustup-init.sha256)
chmod +x "${rustup_tmpdir}/rustup-init"
sudo -u vscode env HOME=/home/vscode "${rustup_tmpdir}/rustup-init" -y \
    --default-toolchain "$rust_version" --profile default --no-modify-path
rm -rf "$rustup_tmpdir"
ln -s /home/vscode/.cargo/bin/rustup /usr/local/bin/rustup
ln -s /home/vscode/.cargo/bin/cargo /usr/local/bin/cargo
ln -s /home/vscode/.cargo/bin/rustc /usr/local/bin/rustc
ln -s /home/vscode/.cargo/bin/rustfmt /usr/local/bin/rustfmt
ln -s /home/vscode/.cargo/bin/cargo-clippy /usr/local/bin/cargo-clippy

# For the pre-commit/CI checks (`cargo audit`, the Makefile's `sbom`
# target) -- cargo-installed, pinned like every other tool in this stage.
sudo -u vscode env HOME=/home/vscode CARGO_TARGET_DIR=/home/vscode/.cache/cargo-target \
    /home/vscode/.cargo/bin/cargo install --locked "cargo-audit@${cargo_audit_version}"
sudo -u vscode env HOME=/home/vscode CARGO_TARGET_DIR=/home/vscode/.cache/cargo-target \
    /home/vscode/.cargo/bin/cargo install --locked "cargo-cyclonedx@${cargo_cyclonedx_version}"

# For the `cargo-llvm-cov` pre-commit hook (tests/README.md's "Coverage
# gate", docs/nfrs/NFR-0023-test-coverage-gate.md) -- needs the
# `llvm-tools-preview` rustup component to instrument coverage, on top of
# the cargo-installed subcommand itself.
sudo -u vscode env HOME=/home/vscode /home/vscode/.cargo/bin/rustup component add llvm-tools-preview
sudo -u vscode env HOME=/home/vscode CARGO_TARGET_DIR=/home/vscode/.cache/cargo-target \
    /home/vscode/.cargo/bin/cargo install --locked "cargo-llvm-cov@${cargo_llvm_cov_version}"

curl -LsSf "https://releases.astral.sh/github/uv/releases/download/${uv_version}/uv-installer.sh" \
    | sudo -u vscode env HOME=/home/vscode INSTALLER_NO_MODIFY_PATH=1 sh
ln -s /home/vscode/.local/bin/uv /usr/local/bin/uv
ln -s /home/vscode/.local/bin/uvx /usr/local/bin/uvx

# python3 isn't otherwise a dependency of this template -- it's
# infrastructure tooling only: prek needs *some* interpreter to build the
# venv for any "language: python" hook, and .github/scripts/*.py need one
# directly. uv is the mechanism for an exact, checksum-verified build
# (python-build-standalone) rather than an unpinned apt package.
sudo -u vscode env HOME=/home/vscode /usr/local/bin/uv python install "$python_version"
python_bin="$(sudo -u vscode env HOME=/home/vscode /usr/local/bin/uv python find "$python_version")"
ln -s "$python_bin" /usr/local/bin/python3
ln -s "$python_bin" /usr/local/bin/python

# Node.js, likewise infrastructure tooling only: the clear-thought MCP
# server in ../.mcp.json is the only thing that needs npx. Installed
# directly from nodejs.org's own release tarballs (checksum-verified
# against that release's published SHASUMS256.txt) rather than a
# devcontainer feature, so every tool this stage installs is pinned the
# same way, in the same place.
node_dpkg_arch="$(dpkg --print-architecture)"
case "$node_dpkg_arch" in
    amd64) node_arch=x64 ;;
    arm64) node_arch=arm64 ;;
    *)
        echo "unsupported architecture for Node.js install: $node_dpkg_arch" >&2
        exit 1
        ;;
esac
node_asset="node-v${node_version}-linux-${node_arch}"
node_tmpdir="$(mktemp -d)"
curl -LsSf -o "${node_tmpdir}/${node_asset}.tar.xz" \
    "https://nodejs.org/dist/v${node_version}/${node_asset}.tar.xz"
curl -LsSf -o "${node_tmpdir}/SHASUMS256.txt" \
    "https://nodejs.org/dist/v${node_version}/SHASUMS256.txt"
(cd "$node_tmpdir" && grep " ${node_asset}.tar.xz\$" SHASUMS256.txt | sha256sum -c -)
mkdir -p /usr/local/lib/nodejs
tar -xJf "${node_tmpdir}/${node_asset}.tar.xz" -C /usr/local/lib/nodejs
rm -rf "$node_tmpdir"
ln -s "/usr/local/lib/nodejs/${node_asset}/bin/node" /usr/local/bin/node
ln -s "/usr/local/lib/nodejs/${node_asset}/bin/npm" /usr/local/bin/npm
ln -s "/usr/local/lib/nodejs/${node_asset}/bin/npx" /usr/local/bin/npx

# prek (https://prek.j178.dev/) is installed as a standalone tool, not a
# project dependency of any particular language's package manager.
curl -LsSf "https://github.com/j178/prek/releases/download/v${prek_version}/prek-installer.sh" \
    | sudo -u vscode env HOME=/home/vscode INSTALLER_NO_MODIFY_PATH=1 sh
ln -s /home/vscode/.local/bin/prek /usr/local/bin/prek

curl -fsSL https://claude.ai/install.sh \
    | sudo -u vscode env HOME=/home/vscode bash -s "$claude_code_version"
ln -s /home/vscode/.local/bin/claude /usr/local/bin/claude

# For the snip Claude Code PreToolUse hook -- see .claude/README.md. Not on
# PyPI/npm/crates.io, so fetched as a release tarball and checksum-verified
# against the project's own published checksums.txt instead of trusting a
# curl-pipe-to-sh installer.
snip_arch="$(dpkg --print-architecture)"
snip_asset="snip_${snip_version}_linux_${snip_arch}.tar.gz"
snip_tmpdir="$(mktemp -d)"
sudo chmod a+rwx "$snip_tmpdir"
curl -LsSf -o "${snip_tmpdir}/${snip_asset}" \
    "https://github.com/edouard-claude/snip/releases/download/v${snip_version}/${snip_asset}"
curl -LsSf -o "${snip_tmpdir}/checksums.txt" \
    "https://github.com/edouard-claude/snip/releases/download/v${snip_version}/checksums.txt"
(cd "$snip_tmpdir" && grep " ${snip_asset}\$" checksums.txt | sha256sum -c -)
tar -xzf "${snip_tmpdir}/${snip_asset}" -C "$snip_tmpdir" snip
sudo -u vscode install -Dm755 "${snip_tmpdir}/snip" /home/vscode/.local/bin/snip
rm -rf "$snip_tmpdir"
ln -s /home/vscode/.local/bin/snip /usr/local/bin/snip

# For connecting to the s3 stack service (RustFS) -- see
# .devcontainer/stack/s3/README.md. Published as .deb/.rpm release assets
# plus a SHA256SUMS file, not on apt/PyPI/npm/crates.io, so fetched and
# checksum-verified the same way as snip above.
rustfs_cli_arch="$(dpkg --print-architecture)"
rustfs_cli_asset="rustfs-cli_${rustfs_cli_version}_${rustfs_cli_arch}.deb"
rustfs_cli_tmpdir="$(mktemp -d)"
curl -LsSf -o "${rustfs_cli_tmpdir}/${rustfs_cli_asset}" \
    "https://github.com/rustfs/cli/releases/download/v${rustfs_cli_version}/${rustfs_cli_asset}"
curl -LsSf -o "${rustfs_cli_tmpdir}/SHA256SUMS" \
    "https://github.com/rustfs/cli/releases/download/v${rustfs_cli_version}/SHA256SUMS"
(cd "$rustfs_cli_tmpdir" && grep " ${rustfs_cli_asset}\$" SHA256SUMS | sha256sum -c -)
apt-get install -y --no-install-recommends "${rustfs_cli_tmpdir}/${rustfs_cli_asset}"
rm -rf "$rustfs_cli_tmpdir"

# `kcadm` reaches the sibling `keycloak` container's Admin REST API from
# the devcontainer -- see .devcontainer/stack/keycloak/README.md. A thin
# curl wrapper rather than the official kcadm.sh, which ships only inside
# Keycloak's full server distribution and needs a JVM neither this stage
# nor the app otherwise requires.
kcadm_tmpfile="$(mktemp)"
sudo chmod a+rwx "$kcadm_tmpfile"
cat > "$kcadm_tmpfile" <<'EOF'
#!/usr/bin/env bash
# Thin curl wrapper for the Keycloak Admin REST API. See
# .devcontainer/stack/keycloak/README.md for usage and required env vars.
set -euo pipefail

if [[ $# -lt 2 ]]; then
    echo "Usage: kcadm METHOD PATH [JSON_BODY]" >&2
    echo "Example: kcadm GET /admin/realms/template-axum/users" >&2
    exit 1
fi

method=$1
path=$2
body=${3:-}
base_url=${KEYCLOAK_URL:-http://keycloak:8080}

token=$(curl -sf -X POST "${base_url}/realms/master/protocol/openid-connect/token" \
    -d grant_type=password \
    -d client_id=admin-cli \
    -d "username=${KEYCLOAK_ADMIN}" \
    -d "password=${KEYCLOAK_ADMIN_PASSWORD}" \
    | python3 -c 'import json, sys; print(json.load(sys.stdin)["access_token"])')

curl_args=(-sf -X "$method" "${base_url}${path}" \
    -H "Authorization: Bearer ${token}" \
    -H "Content-Type: application/json")
[[ -n "$body" ]] && curl_args+=(-d "$body")

curl "${curl_args[@]}" | { python3 -m json.tool 2>/dev/null || cat; }
EOF
sudo -u vscode install -Dm755 "$kcadm_tmpfile" /home/vscode/.local/bin/kcadm
rm -f "$kcadm_tmpfile"
ln -s /home/vscode/.local/bin/kcadm /usr/local/bin/kcadm
