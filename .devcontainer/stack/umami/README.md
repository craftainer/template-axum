# umami

[Umami](https://umami.is) 3.4.1 (MIT), cookieless web analytics. The app
sends events to it server-side (`craftainer-observability`'s `analytics` feature posts batches to `/api/batch`).

- Compose file: `compose.yml` (services `umami`, plus the one-shots `umami-db-init` and `umami-seed`)
- Image: `ghcr.io/umami-software/umami:3.4.1`, pinned by digest
- Host (from other containers): `umami`, port `3000` (forwarded to the host for the dev UI)
- Database: `umami` in the stack's Postgres (`postgres:5432`), created by `umami-db-init` (idempotent; `initdb.d` would skip an existing volume)
- Website: `umami-seed` inserts the site `template-axum` with ID `a076165c-04f6-5693-94ad-a3ae8ca0c316` (UUIDv5 of `template-axum.craftainer.local`, in the DNS namespace) owned by the admin user from Umami's first migration. Idempotent.
- Configuration, next to this file: `umami.env` (the umami service only: its `DATABASE_URL` and `APP_SECRET`; myapp does not load it) and `analytics.env` (the app-facing `ANALYTICS_*` settings: backend, endpoint, site ID, property allowlist `powers_count`, hostname; loaded by myapp and by `umami-seed`, holds no secrets).

## Do

- Open the forwarded port 3000 and log in as `admin` / `umami` (dev only; change it if the port is ever reachable by anyone else).
- Send a User-Agent that is not a bot's with every request to `/api/send`: Umami silently drops requests from `httpx`/`undici` UAs (response `{"beep":"boop"}`).

## Don't

- Expose port 3000 beyond the devcontainer's forwarding. **Ingestion (`/api/send`, `/api/batch`) is unauthenticated**: anyone who can reach it can write events.
- Load `umami.env` into the app, or reuse `APP_SECRET` from it anywhere else. It is a dev-only value.

## Retention

Umami OSS has no retention setting. The CNIL recommends keeping audience-measurement data for at most 25 months. To purge older data (not scheduled; no cron service here), run:

```sh
PGPASSWORD=app psql -h postgres -U app -d umami -f .devcontainer/stack/umami/purge.sql
```

## Removing this service

Delete this directory, remove its entry from `.devcontainer/compose.instance.yml`'s `include:` list and `3000` from `forwardPorts`/`portsAttributes` in `.devcontainer/devcontainer.json`. The `umami` database stays in the Postgres volume until `docker compose down -v`.
