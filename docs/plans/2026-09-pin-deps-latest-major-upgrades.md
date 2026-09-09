# Pin remaining deps to their latest major version

## Status

Draft

## Goal

A "pin all deps to latest" pass pinned every `Cargo.toml` dependency to
its newest version within its *current* major (see the pin comments for
what changed). Six crates have a newer major already on crates.io that
this plan defers because each needs real migration work, not just a
version-number edit:

| Crate | Pinned now | Latest major | Why deferred |
|---|---|---|---|
| `sea-orm` / `sea-orm-migration` | 1.1.16 | 2.0.2 | Pulls in `sea-query` 1.0 (breaking query-builder rewrite) and `sqlx` 0.9; needs Rust ≥1.94.1, newer than this repo's pinned `RUST_VERSION`/`rust-version` (1.91.1) |
| `redis` | 0.32.7 | 1.7.0 | Breaking API change across the client this repo builds on (`src/rate_limit.rs`) |
| `jsonwebtoken` | 9.3.1 | 11.0.0 | Breaking API change in the JWT decode/verify surface `src/oidc/mod.rs` calls directly |
| `tower-http` | 0.6.11 | 0.7.1 | Breaking change to the `trace` layer this repo enables |
| `reqwest` | 0.12.28 | 0.13.5 | Breaking change (confirmed while researching this: the `rustls-tls` feature was renamed `rustls`) |
| `base64` | 0.22.1 | 0.23.1 | Breaking change to the encode/decode API `src/oidc/mod.rs::decode_without_verification` calls directly |
| `getrandom` | 0.3.4 | 0.4.3 | Breaking change to the RNG-fill API `src/events.rs` calls directly |

`aws-sdk-s3`/`aws-config` stay at their original pins (1.112.0 / 1.8.11)
rather than bumping within their current major, even though that bump
is non-breaking per semver: newer `aws-sdk-s3`/`aws-config` transitively
pull in newer `aws-sdk-sts`/`aws-sdk-sso`/`aws-sdk-ssooidc`, which
themselves need Rust ≥1.94.1 with no older-but-still-current release
available — the same Rust-version floor `sea-orm` 2.x hits below. That
gap (and `sea-orm`'s) closes automatically once this plan's Rust bump
lands; no extra work needed there beyond re-running the "pin to latest"
pass.

## Approach

1. **Bump the Rust toolchain first, standalone.** `sea-orm` 2.x's
   `sqlx` 0.9 dependency needs Rust ≥1.94.1. Bump `Dockerfile`'s
   `RUST_VERSION` and `Cargo.toml`'s `rust-version` together, flag it
   for a devcontainer rebuild (only the user can trigger one — see this
   repo's `CLAUDE.md`), and verify the *existing* dependency graph still
   builds under the new compiler before touching any crate version.
2. **One crate (or tightly coupled group) at a time**, each its own
   commit: bump the pin, run `cargo update -p <crate>`, fix compile
   errors, run the check suite, verify. Suggested order — independent
   items first, `sea-orm` last since it's the largest:
   - `getrandom` 0.3→0.4 (`src/events.rs`'s subscriber-id fill call)
   - `base64` 0.22→0.23 (`src/oidc/mod.rs`'s decode call)
   - `jsonwebtoken` 9→11 (`src/oidc/mod.rs`'s decode/verify calls)
   - `tower-http` 0.6→0.7 (the `trace` layer wiring, likely
     `src/lib.rs` or `src/telemetry.rs`)
   - `redis` 0.32→1.x (`src/rate_limit.rs`)
   - `sea-orm`/`sea-orm-migration` 1.x→2.x (`src/repositories/hero_sea_orm.rs`,
     `src/crud/mod.rs`, `src/repositories/filtering.rs`,
     `src/migration/`) — by far the largest surface; consider whether
     it's worth its own plan document once the Rust-bump item above
     lands and the actual diff size is known.
   - `reqwest` 0.12→0.13 (`src/oidc/mod.rs`'s HTTP client construction;
     the feature rename `rustls-tls`→`rustls` is already known from
     this pass's research)
3. For each crate, check its own changelog/migration guide (fetch it
   fresh — don't rely on training-data knowledge of the API shape, per
   this repo's `CLAUDE.md` "look up library/framework documentation"
   rule) before editing call sites.
4. After the full set lands, re-run a "pin to latest" pass once more —
   crates release continuously, so some of today's "latest majors" will
   likely have moved again by the time this plan is picked up.

## Open questions

- Whether `sea-orm` 2.x is worth its own follow-up plan document (item
  is large enough that scoping it in detail now, before the Rust bump
  even lands, may just go stale).
- Whether to bump the Rust toolchain in the same PR as the first crate
  bump that needs it (`sea-orm`) or genuinely standalone, ahead of any
  crate change, so a bad interaction is easy to bisect.
