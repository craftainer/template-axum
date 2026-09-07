# keycloak

Keycloak, the OIDC identity provider the API authenticates against (validates against any PKCE-capable OIDC provider,
not just Keycloak — see phase 2's OIDC integration). Starts in dev mode and
auto-imports `realm-export.json`, so a working realm, client, and test user
exist immediately — no manual admin-console setup required.

- Compose file: `compose.yml`
- Image: `quay.io/keycloak/keycloak:26.7.3`
- Host (from other containers): `keycloak`
- Port (container-internal): `8080`
- Realm: `template-axum`
- Client ID: `api` (public client, no client secret)
- Admin console user / password: `admin` / `admin`
- Test users / password (same as username): `viewer`, `editor`, `security`,
  `maintainer`, `detective` — all with the same `offline_access` /
  `uma_authorization` realm roles, plus one `api` client role each
  matching their username (`clientRoles` in `realm-export.json`) —
  what each role grants is up to the app's own routes (see phase 2's
  OIDC/RBAC integration).
- Issuer URL (from other containers): `http://keycloak:8080/realms/template-axum`
- Admin credentials and the `OIDC_*` URLs: defined in `keycloak.env`, next to this file

Unlike the other stack services, Keycloak's port is listed in
`forwardPorts` in `.devcontainer/devcontainer.json` so you can reach the
login and admin UI from your host browser at `http://localhost:8080`. Since
Keycloak runs in a sibling container rather than the primary `api` one, that
entry must use the `"keycloak:8080"` (`serviceName:port`) form, not a bare
`8080` — a bare port number only forwards from the primary container. That
still isn't a compose `ports:` mapping — see the root README's "Don't" for
why the two are different.

From inside the devcontainer, `kcadm` (installed by `scripts/develop.sh`) is
a thin curl wrapper around the Admin REST API — a lighter alternative to the
official `kcadm.sh`, which ships only inside Keycloak's full server
distribution and needs a JVM neither this stage nor the app otherwise
requires. It logs in as `KEYCLOAK_ADMIN`/`KEYCLOAK_ADMIN_PASSWORD` (from
`keycloak.env`, already in the devcontainer's environment) and takes an
HTTP method, an Admin REST API path, and an optional JSON body:

```bash
kcadm GET /admin/realms/template-axum/users
kcadm PUT /admin/realms/template-axum/users/<id> '{"firstName": "Dev"}'
```

## Do

- Use the test users or `admin` for local development only.
- Edit `realm-export.json` and recreate the container (`docker compose up -d
  --force-recreate keycloak`, or rebuild the devcontainer) to change the
  realm, add clients, or add users — it's re-imported on every start.

## Don't

- Add a `ports:` entry to this file; use `forwardPorts` in
  `devcontainer.json` instead (see above).
- Reuse these defaults, or `realm-export.json` as written, anywhere but
  local dev.

## Removing this service

Delete this directory, remove its compose file entry from
`.devcontainer/compose.yml`'s `include:` list, and remove
`"keycloak:8080"` from `forwardPorts`/`portsAttributes` in
`devcontainer.json` too.
