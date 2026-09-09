# Close remaining template-fastapi parity gaps

## Status

Draft

## Goal

`template-axum` is a feature-by-feature Rust/axum port of
`template-fastapi` (see root `README.md`). Comparing this repo's
`docs/frs/`, `docs/nfrs/`, `docs/adrs/`, `tests/README.md`, and
`Cargo.toml` against the reference repo's turned up seven features the
reference has that this port doesn't yet, most of them already
self-documented here as deferred (`NFR-0024`, `NFR-0026`, ADRs 0012/
0014/0015 all say "Tier C item 6/7, out of scope"). This plan is the
follow-up those notes point at: port the remaining features, in
dependency order, so every FR/NFR this repo carries can move to
`Implemented`.

Not a gap: `NFR-0006` (flat-only XML codec) and `NFR-0016` (no
connection pooling) are Python/asyncpg-specific constraints this port's
different libraries (`quick-xml` serde, `sea-orm`'s pool) make moot —
no action needed. Valkey-over-Redis (reference ADR-0013) is already
the stack's choice (`.devcontainer/stack/redis/compose.yml` runs
`valkey/valkey`); only the ADR recording it is missing, not worth a
plan item on its own.

## Approach

Items 1-3 are ordered (each depends on the previous); items 4-7 are
independent of each other and of 1-3.

1. **Hero v1 deprecated-compat API** (ports reference `FR-0005`,
   `FR-0006`; this is "Tier C item 7" named throughout `docs/adrs/0012`,
   `0014`, `0015` and `docs/nfrs/NFR-0026`).
   - Add `views::hero_v1` (single `superpower: String` field) and
     `controllers::heroes_v1`, mounted at `/v1/heroes*`, backed by the
     *same* `HeroSeaOrmRepository`/`HeroMemoryRepository` v2 data — no
     new table.
   - Conversion: v2→v1 takes `powers[0]` as `superpower` (lossy,
     document it); v1→v2 create wraps `superpower` into a
     single-element `powers` list; v1→v2 update maps `superpower` to
     `powers` only when the client actually supplied `superpower`,
     never clobbering existing `powers` when it's omitted.
   - New `FR-000X`/`FR-000Y` docs (ported from reference `FR-0005`/
     `FR-0006`) plus an ADR if the conversion approach needs recording
     beyond what the FRs state.

2. **Sunset/Deprecation headers on the v1 routes** (moves
   `NFR-0026` from `Proposed` to `Implemented`; ports reference
   `NFR-0002`).
   - Every `/v1/heroes*` handler (JSON, and XML once item 3 lands)
     returns `src/http_headers.rs::Sunset` as part of its response
     tuple, per the mechanism `ADR-0012` already built and left
     unapplied. Current-version (`/v2`) routes carry none of these
     headers — add a test asserting that split.
   - Update `NFR-0026`'s Status to `Implemented` and its Verification
     section per its own text ("add the same automated assertion the
     reference makes").

3. **Hero v1 XML representation** (ports reference `FR-0008`'s v1 half;
   `FR-0027`'s doc already scopes this in: "XML router... only for the
   deprecated Hero v1").
   - Extend the `heroes_xml` sibling-router pattern (`ADR-0014`) to
     `/v1/heroes*`, reusing `hero_v1`'s flat shape (already
     XML-friendly — no nested fields).

4. **Audit endpoint** (ports reference `FR-0016`). The `security`/
   `detective` roles already exist in the role matrix
   (`FR-0015`, `src/oidc/mod.rs`'s tests) but nothing consumes them.
   - Add `GET /audit`, restricted to `security` or `detective`
     (403 otherwise), returning the caller's subject and granted
     roles from `Claims`.
   - New `FR-000Z` doc.

5. **HTML form CRUD with progressive enhancement** (ports reference
   `FR-0009`). Targets the existing v2 JSON endpoints directly, so it
   has no dependency on items 1-3.
   - `views::hero_form`/`controllers::heroes_web` (naming to match this
     repo's `controllers`/`views` split) serving `GET /heroes/form` as
     a working no-JS `<form>`, plus `GET /heroes/components.js` for
     vanilla-JS progressive enhancement against the same JSON endpoints.
   - `powers` submits/renders as a comma-separated string; a validation
     failure returns 422 (`FR-0018`'s existing Problem Details path);
     success responds `303 See Other` back to the form.
   - New `FR-000W` doc.

6. **Optional OTLP log export** (ports reference `FR-0023`; already
   flagged as deferred-not-forgotten in `docs/adrs/0006`, lines 38-41).
   - When `OTEL_EXPORTER_OTLP_ENDPOINT` is set, add an OTLP log-export
     `tracing_subscriber::Layer` via a batch processor, using
     OpenTelemetry's own env-var conventions (no new app setting).
   - Update `ADR-0006`'s "No OTLP log export exists in this phase" note
     once landed.

7. **Remaining test tiers + release smoke-test gate** (moves
   `NFR-0024` from `Proposed` to `Implemented`; ports reference
   `NFR-0021` and `NFR-0010`).
   - **e2e-equivalent tier**: `reqwest` driving one live `MODE=dev`
     process and one live `MODE=mock` process, role-journey style
     (mirrors the reference's per-role `tests/e2e/` split) — per
     `tests/README.md`'s "What's not here yet".
   - **perf tier**: a load test against the `runner`-stage image only
     (never `dev`/`mock`), per `docs/adrs/0010`'s reasoning; pick a
     Rust-appropriate tool (the reference uses Locust; this repo isn't
     bound to that choice — worth its own ADR if it departs).
   - **Release smoke-test gate**: before a release counts as verified,
     run the built `runner` image against real Postgres/Redis/S3/
     Keycloak and poll `/health/ready` (5s interval/timeout, 30
     retries, 150s start period) until healthy, wired into
     `.github/workflows/release.yml`.
   - Update `NFR-0024`'s Status and Verification once all four tiers
     have a CI step.

## Open questions

- Item 1's new FR numbers: this repo's FRs currently stop at
  `FR-0030` — confirm the next free numbers before writing the docs
  (`FR-0031`+ for items 1, 4, 5; a numbering collision is the one thing
  worth double-checking here since FRs are never renumbered).
- Item 7's load-test tool choice (Locust parity vs. a Rust-native
  alternative like `goose` or `drill`) needs its own decision before
  work starts — flag for an ADR either way.
