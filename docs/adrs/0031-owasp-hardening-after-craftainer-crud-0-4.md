# 0031. Harden the event bus, the demo form's auth, the write limiter and the startup mode after the OWASP review

## Status

Proposed

Amends [ADR 0016](0016-mqtt-backed-sse-crud-event-stream.md),
[ADR 0011](0011-hand-rolled-redis-rate-limiter-over-tower-governor.md) and
[ADR 0007](0007-owner-scoped-hero-example-resource.md); supersedes the
`?token=` scope decision recorded in
[ADR 0030](0030-resource-server-metadata-security-headers-and-api-catalog.md)
and in `controllers::heroes_web`'s earlier module doc.

## Context

The OWASP review of this template (OW-template-axum, at v1.8.4) found six
Medium issues and a set of Low ones, and the libraries it depends on shipped
the pieces needed to fix several (`craftainer-crud` 0.4.0,
`craftainer-observability` 0.5.0). The findings this ADR answers:

- **MQTT had no transport security.** The bus connected anonymously over
  plaintext, `Settings::validate` enforced TLS for Postgres, S3, Redis and
  OIDC but deliberately not MQTT, and the dev broker allows anonymous
  clients. Any peer on the network could read every CRUD event or publish
  forged ones (A02).
- **`?token=` on the demo form page.** The bearer token went into the URL,
  every form `action`, the script `src` and the redirect `Location`, so it
  landed in browser history, proxy and access logs and in the `uri` field of
  the request span. RFC 6750 section 2.3 says a URI query token SHOULD NOT be
  used (A04).
- **The write rate limit ran before the role check and was keyed on the peer
  address.** Behind the BFF every user shares one address, so one `viewer`
  (whose writes are refused anyway) could drain the single bucket for every
  editor (A06).
- **`MODE` defaulted to `dev`.** `Mode::Dev` skips every production check, so a
  deployment that forgot `MODE=production` started on default credentials over
  plaintext with internal error detail returned to clients; the strict
  validation was opt-in, i.e. fail-open (A02).
- **The OIDC discovery and JWKS fetches had no timeout** and followed
  redirects, so a stalled identity provider tied up every authenticated
  request (A10).
- **Reads are open and archived rows were readable by anyone.**
  `include_archived` and `only_archived` return every owner's soft-deleted
  rows to any caller who passes them; `craftainer-crud` leaves gating them to
  the host (A01).
- `craftainer-crud` 0.4.0 also changes how events are delivered and streams
  held: after-commit best-effort delivery (`PendingEvents`), at most 5 streams
  per caller and 2 per subscriber id, transient streams unless the host opts
  into durable ones, and `*_for_caller` routes.

## Decision

1. **MQTT is secured with TLS and credentials, required in production.**
   `craftainer-crud` is built with `mqtt-tls` (rumqttc over the system
   OpenSSL, not rustls: `use-rustls` pulls `rustls-webpki` 0.102 and an
   unmaintained `rustls-pemfile`, both with open advisories). The build image
   needs `pkg-config` and `libssl-dev` (`scripts/builder.sh`, and
   `scripts/develop.d/30-libssl.sh` in the devcontainer) and the runtime image
   `libssl3` (`scripts/runner-setup.d/20-libssl3.sh`). `MQTT_USERNAME`,
   `MQTT_PASSWORD`, `MQTT_TLS_CA_FILE` (the only trust anchor; setting it turns
   TLS on, TLS 1.2 or later, host name always verified), `MQTT_TLS_CLIENT_CERT`
   and `MQTT_TLS_CLIENT_KEY` (mutual TLS, PKCS#8 PEM) and `MQTT_ALLOW_INSECURE`
   are read by `config::mqtt_security`, held outside `Settings` like
   `SUBSCRIBER_SECRET`, and wired by `build_event_bus`.
   `Settings::from_env` rejects a half-set pair, a client certificate without a
   CA file, and credentials over plaintext without `MQTT_ALLOW_INSECURE=true`
   in every mode; `MODE=production` additionally requires the CA file and
   credentials and refuses `MQTT_ALLOW_INSECURE`. The bus also validates its
   own settings at startup (`EventBus::validate`). The dev broker is left as
   it was (anonymous, plaintext; it is `MODE=dev`, which sets none of the
   variables): hardening the dev stack's broker is a follow-up.
2. **Events are delivered after the write, through `PendingEvents`.** Every
   mutating handler calls `controllers::announce`, which records the event in
   a `PendingEvents` and commits it on the bus in its own task, so a client
   that disconnects mid-request cannot drop the event of a committed write.
   Each hero write is one statement (ADR 0028), so there is no transaction to
   roll back and nothing is announced before the write has succeeded.
3. **Event streams are transient and capped per caller.** `/events` calls
   `event_stream_until_for_caller` with the token's `sub`, so a caller holds at
   most 5 concurrent streams (and 2 per subscriber id), `429` beyond. Streams
   are transient (clean session, no replay; a `subscriber_id` or
   `Last-Event-ID` the client sends back is ignored) because the bus is not
   built `with_durable_streams`; the manifest therefore advertises
   `Replay::None`. Durable sessions are an opt-in this template does not
   enable.
4. **The demo form page no longer takes a token in the URL.** Closest standard
   pattern for a page a browser navigates to without script: the user pastes a
   token into a form that `POST`s it to `/heroes/form/session` (RFC 6750
   section 2.2, a form-encoded body); the app verifies it and sets the
   `hero_session` cookie (`HttpOnly`, `SameSite=Strict`, `Path=/heroes`,
   `Secure` when `PUBLIC_BASE_URL` is https, one hour). `WebAuthClaims` accepts
   that cookie or an `Authorization: Bearer` header; nothing else. The token is
   verified on every request, so the cookie cannot outlive the token's `exp`.
   Because a cookie is ambient authority, an unsafe request authenticated by it
   (and the session routes) is refused when `Sec-Fetch-Site` says it is
   cross-site, on top of `SameSite` (OWASP CSRF cheat sheet, fetch-metadata
   defence). The page renders no token anywhere, is served `Cache-Control:
   no-store`, and has a sign-out form (`POST /heroes/form/logout`). The
   progressive-enhancement script submits the same form routes with `fetch`
   and the same cookie instead of calling the JSON API with a token it can read
   (an `HttpOnly` cookie is by design unreadable to script). The JSON API
   itself stays bearer-only.
5. **The write limit is charged after the role check and keyed on `sub`.**
   `controllers::enforce_write_limit` replaces the five copies of the
   limiter call: it runs after `require_any_role`, and the key is the verified
   subject instead of the peer address. `POST /mock/token`, which has no
   subject, stays keyed on the peer address.
6. **`MODE` has no default.** `Settings::from_env` fails when it is unset (or
   empty, or not UTF-8). The devcontainer's `myapp` service, the smoke stack
   (`image.mk`), the perf stack and the e2e/integration tests set it
   explicitly. The Dockerfile sets none.
7. **Discovery and JWKS fetches are bounded and do not follow redirects**
   (5 s in all, 2 s to connect). A stalled provider now fails requests as `503`
   after that long instead of holding them open. In the same place: the
   `Bearer` scheme is matched case-insensitively (RFC 9110 section 11.1) and
   `nbf` is validated.
8. **Archived rows are readable only with `maintainer`.** A request for
   `include_archived=true` or `only_archived=true` (list, single read, `/stats`
   and the `If-Match` pre-read of a write) from any other read role is `403`
   (`controllers::require_archive_access`, after the flags parse so a malformed
   flag stays `422`). The decision text says "admin"; this template's role
   matrix (FR-0015) has no such role and `maintainer` is its most privileged
   one, so `HERO_ARCHIVE_READ_ROLES` is `["maintainer"]`. Introducing a
   dedicated role is a change to that one constant plus the realm.
9. **Analytics follows `craftainer-observability` 0.5.0.** `AnalyticsConfig::
   from_env()` returns a `Result`; `main` reads it at startup and exits 1 on an
   unknown `ANALYTICS_*` value, then installs it with `analytics::init` (and the
   telemetry handle, so the `otel` backend can find the logger provider).
   Per-request consent and GPC come from `RequestContext::from_headers`:
   a repeated `Analytics-Consent` header counts only if every copy is `1`. The
   header is a bare claim: under `ANALYTICS_CONSENT=opt-in` the BFF must strip
   the client's copy and set its own.

## Consequences

- A deployment must now set `MODE`, and in production the MQTT variables; a
  mistake stops the process at startup with a message naming the variable.
  Existing deployments (and the devcontainer, which needs a rebuild to pick up
  `MODE: dev`) must add them.
- The runtime image gains `libssl3`; the build and devcontainer images gain
  `libssl-dev` and `pkg-config` (a devcontainer rebuild).
- Event streams lose resumption. Clients that relied on `Last-Event-ID` replay
  refetch instead; the manifest says so.
- The demo page's flow changes: the first visit asks for a token and keeps it
  in a cookie. Anything that linked to `/heroes/form?token=` must use the
  bearer header or the session form.
- `If-Match`/optimistic concurrency (`lib-crud-rs` 0.5, a breaking repository
  change), a TLS listener and authentication on the dev broker, request/body
  limits at the HTTP edge are not part of this change.
- `libsqlite3-sys` 0.30.1 is in `Cargo.lock` only through optional weak
  dependency features of `sqlx`; `cargo tree` for the whole workspace, all
  features and all targets does not reach it, so RUSTSEC-2026-0459 is not
  compiled in. `osv-scanner.toml` carries a dated ignore until 2027-01-09 for
  the lockfile-based scan, to be removed when a `sqlx` release allows
  `libsqlite3-sys` 0.39.

## Addendum: follow-ups (JWKS refetch, session limit, `SUBSCRIBER_SECRET`)

1. **JWKS: single flight, unknown-`kid` refetch, cooldown.** A token whose
   `kid` the cached key set lacks (what a signing-key rotation looks like)
   refetches the key set instead of waiting out the 5-minute cache. Refetches
   are serialised behind one lock held for the whole fetch, and a request that
   queued behind a fetch uses its outcome instead of fetching again, so ten
   concurrent unknown-`kid` requests cost one fetch. No attempt starts within
   30 s of the last one (successful or not), so attacker-chosen `kid`s cannot
   make the app hammer the identity provider; an unknown `kid` after the
   refetch (or inside the cooldown) is `401`. A failed fetch keeps the cached
   keys serving, for up to an hour past their 5-minute expiry; with nothing
   usable cached the answer is `503`. The first fetch after start-up is exempt
   from the cooldown (nothing to fall back on) but still single-flight. The
   30 s and one-hour bounds are constants in `oidc/mod.rs`, not settings.
2. **`POST /heroes/form/session` is rate-limited.** It is the one route that
   verifies a caller-supplied token before there is any identity, so it is
   limited per peer address (10 per minute, the same limiter and fixed window
   as `POST /mock/token`), after the cross-site check and before the token is
   looked at. `429` responses (here and for the hero write limit and
   `POST /mock/token`) now carry `Retry-After: 60`, the window length. Behind
   a proxy or BFF the peer address is the proxy's, so the budget is shared by
   everyone it fronts; the page is meant for direct use, and the JSON API
   (keyed on `sub`) is unaffected.
3. **`SUBSCRIBER_SECRET` is removed (SUBSEC).** It signed the subscriber ids
   `/events` mints so a client could resume a *durable* session on any
   replica. Streams are transient and a client-supplied id is ignored
   (decision 3), so no deployment can use the signature for anything: a
   random per-process key is equivalent, and requiring a 32-character secret
   in production was configuration with no effect (and a reason for a
   deployment to fail to start).

   Confirmed against `craftainer-crud` 0.4.0 (the version in `Cargo.lock`,
   tag `v0.4.0`): the route calls `crud_events::event_stream_until_for_caller`,
   whose `open_stream` verifies a presented id (`resolve_subscriber_id`) and
   then calls `EventBus::subscribe_for_caller` (`src/events.rs`), which begins
   `let requested = requested.filter(|_| self.durable_streams);` and opens the
   MQTT session with `set_clean_session(!durable)`. `durable_streams` is
   `false` unless the host calls `with_durable_streams`, which this template
   never does, so a presented id, signed or not, is dropped before the
   session is opened and a fresh id is minted. The other subscriber, the
   stats-cache invalidation task (`src/cache.rs`) uses `subscribe_transient(.., None)`.

   Removed: `config::subscriber_secret`, its start-up check in `main`, the
   `subscriber_secret` parameter of `build_event_bus` (now
   `build_event_bus(settings, &mqtt)`), the production requirement and the two
   failure-mode tests. A set `SUBSCRIBER_SECRET` is ignored, whatever its
   value, with one deprecation warning at start-up
   (`config::subscriber_secret_is_set`), so existing deployments keep
   starting. If durable streams are ever enabled, a shared key becomes
   meaningful again (`EventBus::with_subscriber_secret`) and must come back
   with it. This supersedes the "Addendum: `SUBSCRIBER_SECRET`" in ADR 0020.
