# MQTT CRUD event stream, and raising the coverage floor

## Status

Draft

## Goal

Close out the two items deliberately deferred from
`2026-09-tier-b-c-app-features.md` (commits `f039466`..`c560d5b`): the
MQTT-backed SSE event stream (that plan's item 6, skipped for capacity
reasons, not because it's unneeded), and pushing `src/`'s line coverage
from its current 87.78% up toward 100% — both closing the specific gaps
that plan's implementers flagged and revisiting
`docs/adrs/0010-80-percent-line-coverage-floor-via-cargo-llvm-cov.md`'s
floor, which that ADR itself says should happen "as this instance's
Tier B/C (integration, e2e) tests are added."

Each item below should leave `cargo build`/`cargo test`/`cargo clippy
--all-targets -- -D warnings`/`cargo fmt --check`/`cargo llvm-cov
--fail-under-lines <current floor>` all green before moving to the next,
same as the plan this one continues from.

## Approach

Work in the order below — the coverage-floor work in item 2 is easiest
to do accurately once item 1's new code exists and is itself tested, so
it isn't competing with a moving target.

1. **MQTT CRUD event stream.** Reference (read in full — the
   persistent-session/QoS-1 delivery guarantee is the entire point):
   `/workspace/docs/adrs/0015-mqtt-for-crud-events.md`. Verify
   `.devcontainer/stack/mqtt/` is wired into `.devcontainer/compose.yml`'s
   `depends_on` (already copied into template-axum in phase 1 per the
   prior plan). Before starting, confirm `rumqttc` covers persistent
   sessions + QoS 1 cleanly from async Rust (vs. `paho-mqtt` bindings) —
   this was flagged as an open question and never resolved since the
   prior plan didn't reach this item. Implement
   `GET /crud/v1/heroes/v2/json/events` as an SSE stream backed by a
   persistent MQTT session via `rumqttc`, with the same subscriber-id/
   reconnect-replay semantics as the reference. `MODE=mock` gets a
   `tokio::sync::broadcast`-based in-memory fake instead, matching
   `InMemoryEventSink`'s role, so the stack still works with zero
   containers. Write this instance's own ADR/FR/NFR (continuing the
   numbering in `docs/adrs/`, `docs/frs/`, `docs/nfrs/`) documenting the
   actual crate/design choices made — don't guess at the reference's
   Python semantics from memory, read
   `/workspace/docs/adrs/0015-mqtt-for-crud-events.md` in full first. If
   a partial/half-working implementation is the realistic outcome (per
   the prior plan's own warning that this is worse than not attempting
   it), stop and report rather than committing something half-done.

2. **Raise the coverage floor toward ~100%.** Two parts:
   - Close the specific gaps the prior plan's implementation left
     un-covered rather than synthetically padding line count:
     `src/repositories/hero_sea_orm.rs`'s filter/sort/bulk query-building
     and the `/stats` aggregate reads are currently unit-tested only at
     the pure-function/SQL-text level, never against a live Postgres —
     add an integration tier that runs them against the project's own
     `.devcontainer/stack/postgres/` service (per this repo's rule of
     using already-running backing services, not ad-hoc containers).
     The `/predict` happy path (an actual multi-bucket forecast, not
     just the 422-under-two-buckets path) is currently verified only via
     `crud_stats::tests` synthetic series, not end-to-end through HTTP,
     because the test harness can't backdate `created_at` — give the
     harness a way to seed rows with a controlled `created_at` (test-only
     helper, not a production code path) and add the missing end-to-end
     case.
   - Once those gaps are closed, re-run `cargo llvm-cov` and set
     `--fail-under-lines` in `.pre-commit-config.yaml` (and anywhere else
     it's wired) to a number close to what's actually achieved — not
     hardcoded to 100 in advance. Update
     `docs/adrs/0010-80-percent-line-coverage-floor-via-cargo-llvm-cov.md`
     in place (its own "Consequences" section already anticipates this
     revision) to record the new floor and why, rather than writing a
     superseding ADR for what's the same decision at a new number. Note
     any lines that remain deliberately uncovered (e.g. truly
     unreachable `match` arms, `Debug`-only derives) and why chasing
     literal 100% isn't worth it there, rather than forcing coverage of
     code that doesn't warrant a test.

## Open questions

- MQTT: `rumqttc` vs. `paho-mqtt` for persistent sessions + QoS 1 — must
  be resolved before starting item 1 (see above).
- Coverage: the realistic ceiling depends on what's actually
  untestable-without-mocking-the-compiler-guarantees-away in this
  codebase; don't commit to a specific target number until item 2's gap
  analysis is done.
