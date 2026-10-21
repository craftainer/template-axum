#!/usr/bin/env bash
# Installs the CLIs for talking to this instance's backing-services stack
# (.devcontainer/stack/): psql, redis-cli, RustFS's `rc` and `kcadm`.
# Runs as root during the `develop` stage build (see README.md here).
set -euo pipefail

# renovate: datasource=github-releases depName=rustfs/cli
RUSTFS_CLI_VERSION=0.1.32

# postgresql-client/redis-tools give psql/redis-cli for the stack's
# postgres/redis services (see .devcontainer/stack/postgres and
# .devcontainer/stack/redis's READMEs). Debian's own repo only carries one
# version of each -- no exact-version pin available via apt; a newer client
# talking to an older server is standard practice for both.
apt-get update
apt-get install -y --no-install-recommends postgresql-client redis-tools
apt-get clean
rm -rf /var/lib/apt/lists/*

# For connecting to the s3 stack service (RustFS) -- see
# .devcontainer/stack/s3/README.md. Published as .deb/.rpm release assets
# plus a SHA256SUMS file, not on apt/PyPI/npm/crates.io, so fetched and
# checksum-verified the same way as snip above.
rustfs_cli_arch="$(dpkg --print-architecture)"
rustfs_cli_asset="rustfs-cli_${RUSTFS_CLI_VERSION}_${rustfs_cli_arch}.deb"
rustfs_cli_tmpdir="$(mktemp -d)"
curl -LsSf -o "${rustfs_cli_tmpdir}/${rustfs_cli_asset}" \
    "https://github.com/rustfs/cli/releases/download/v${RUSTFS_CLI_VERSION}/${rustfs_cli_asset}"
curl -LsSf -o "${rustfs_cli_tmpdir}/SHA256SUMS" \
    "https://github.com/rustfs/cli/releases/download/v${RUSTFS_CLI_VERSION}/SHA256SUMS"
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
