# 0028. Hero writes are single `UPDATE ... RETURNING` statements; bulk operations are set-based

## Status

Proposed

## Context

`HeroSeaOrmRepository` did every write as read-modify-write:

- `update` and `delete` ran a `SELECT` (by id, owner and `archived_at IS
  NULL`) and then an `UPDATE` of the row they had read.
- `update_many` and `delete_many` ran one `SELECT` for the matching rows and
  then one `UPDATE` per row, so a bulk call of N rows cost N + 1 round
  trips, each in its own implicit transaction.

Two things were wrong with that besides the count. The visibility check and
the write were separate statements, so a delete (or an update by another
request) landing between them was overwritten as if the row had still been
live. And a bulk call that failed part-way left some rows changed and some
not.

Decision log, 2026-10-30 (AX15): run it after the AX14 cache A/B and keep it
on correctness (single-statement `UPDATE ... RETURNING`, set-based
`update_many`/`delete_many`) even without a measured throughput win.

## Decision

Each write is one statement built with SeaORM's `Entity::update_many()`, a
`SET` list from an `ActiveModel` (only the fields the request carries, plus
`updated_at`; the soft delete sets `archived_at` and `updated_at`), the same
`WHERE` the `SELECT` had, and `exec_with_returning` (Postgres `RETURNING *`,
sea-orm 2.0.2):

| Method | `WHERE` | Result |
| --- | --- | --- |
| `update` | `id`, `owner_id`, `archived_at IS NULL` | first returned row, `None` on no match |
| `delete` | `id`, `owner_id`, `archived_at IS NULL` | `true` if a row was returned |
| `update_many` | `archived_at IS NULL` and the filters | returned rows |
| `delete_many` | `archived_at IS NULL` and the filters | returned rows |

`apply_filters` became generic over `QueryFilter` so the same filter mapping
serves `Select` and `UpdateMany`. `RETURNING` does not order rows, so the bulk
methods sort by `id` (what the per-row loops returned in practice).

Behaviour is unchanged: the same values (`None` / `false` / empty `Vec` on a
miss), the same filters and owner scoping, and the same `updated_at` stamping
that the ETag is hashed from (`docs/adrs/0023`). The repository has no
version check, revision or audit side effect of its own. Events are published
by the handlers from the returned rows, as before, and the bulk size cap
(`docs/nfrs/NFR-0027`) is enforced in the controller; neither moved. The
transaction boundary is stronger: one statement is atomic, so a bulk write
now changes all matching rows or none.

Before and after, statements sent to Postgres (`tests/postgres_hero_round_trips.rs`
counts them with sea-orm's metric callback):

| Operation | Before | After |
| --- | --- | --- |
| `update`, row found | 2 | 1 |
| `update`, miss | 1 | 1 |
| `delete`, row found | 2 | 1 |
| `delete`, miss | 1 | 1 |
| `update_many`, N rows | N + 1 | 1 |
| `delete_many`, N rows | N + 1 | 1 |

Nothing was kept as-is: every path could be expressed as one statement
without changing what it returns.

## Consequences

- Lost-update window closed for visibility: an update can no longer write to a
  row archived after it was read. `tests/postgres_hero_round_trips.rs` races
  updates against a delete and checks the archived row is never touched
  afterwards.
- A bulk call holds its row locks for one statement instead of taking them
  row by row, and a large `update_many` is one longer statement rather than N
  short ones. The controller's size cap bounds it.
- No throughput gain is claimed. Whether the saved round trips show up under
  load is a separate benchmark run; the change stands on the correctness
  properties above (decision log).
- The `Mode::Mock` repository is untouched: it already mutates under one lock.

## Measured effect (A/B, 2026-10-30)

The benchmark run that Consequences defers to. Result: no throughput win
that clears the acceptance rule, a clear drop in single-row write latency,
and about 12% fewer transactions per request. The change stays on
correctness regardless (decision log, 2026-10-30).

Method:

- Before (B): image `template-axum:l2-457897b` (id `adcb0cb52b06`), built
  from `457897b`, main before this change. After (A): image
  `template-axum:l2-7aa96c1` (id `cbfe1cd42655`), built from `7aa96c1`
  (#27). The only difference under `src/` is `hero_sea_orm.rs`. Both are
  the `runner` stage, fat LTO, no build overrides.
- `tests/perf/run-real-stack.sh <image> --users 20 --run-time 60s` (20 s
  warm-up excluded, goose `HeroJourneyReal`: weights list 6, stats 2,
  health 3, create/update/delete 1), real stack (dev-container Postgres,
  Redis, MQTT, Keycloak), app capped at 1 vCPU and 2 GB, stats cache off,
  telemetry off. A fresh database is seeded with 500 heroes for every run.
- Five runs per arm in a fixed randomised order (seed 20261030):
  `B0 A A B A A B B B A B B9`, where `B0` and `B9` are extra B runs at the
  start and end as drift checks. Nothing else ran on the host.
- Acceptance rule (plan): real only if the median difference is larger than
  the larger spread (range / median) and at least 3%.
- About 21% of requests are POST/PATCH/DELETE, so single-row update and
  delete are exercised. Bulk operations are not part of the scenario.

Results (medians of the five runs per arm; ms are goose's integer
milliseconds):

| Metric | B (before) | A (after) | A vs B | Spread B / A |
| --- | --- | --- | --- | --- |
| requests/s | 2442.9 | 2503.6 | +2.5% | 3.6% / 2.5% |
| CPU ms/request | 0.408 | 0.398 | -2.5% | 3.2% / 2.3% |
| avg cores | 0.997 | 0.997 | 0 | 0.5% / 0.1% |
| PATCH p50 / p95 / p99 (ms) | 12 / 15 / 17 | 7 / 10 / 12 | -42% / -33% / -29% | |
| DELETE p50 / p95 / p99 (ms) | 12 / 16 / 17 | 7 / 10 / 12 | -42% / -38% / -29% | |
| POST p50 / p95 / p99 (ms) | 7 / 10 / 11 | 7 / 11 / 12 | unchanged | |
| overall p50 / p95 / p99 (ms) | 6 / 20 / 23 | 7 / 21 / 25 | see below | |
| Postgres transactions per request | 1.620 | 1.432 | -11.6% | 0.8% / 0.4% |

Requests/s per run: B 2444.9, 2442.9, 2450.2, 2437.0, 2362.4; A 2503.6,
2486.8, 2515.5, 2456.4, 2518.1. Drift runs: B0 2442.0, B9 2423.6 (-0.8%).
All runs had 0 errors and ran CPU-bound at the 1 vCPU cap (0.99 to 1.00
cores). Transactions per request is `pg_stat_database.xact_commit` for the
perf database after the run divided by the run's request count; it includes
migration and seeding transactions (identical in both arms), so read the
ratio, not the absolute value. `tup_updated` per run tracks the request count
(27.3k to 28.9k) in both arms.

Conclusion:

- Throughput: +2.5% is below both the 3% threshold and the larger spread
  (3.6%, driven by the B run in position 11, which swapped in 19,400 pages, about
  76 MB; two A runs swapped in 22,000 and 27,000 pages without a visible
  loss, the other compared runs under 6,500; the two drift runs swapped in 76,000 and
  20,000 pages). Without that B run the B range is
  0.5% wide, but the difference is still under 3%. By the rule this is
  within noise.
- Write latency: PATCH and DELETE medians fall from 12 ms to 7 ms, matching
  POST, and the A runs never overlap the B runs at p50 (all five A runs 7,
  all five B runs 12). One round trip saved is about 5 ms at p50 here.
- Overall p50 and p95 read 1 ms higher for A, and so do the reads: list p50
  6 to 7 ms, stats p50 19 to 20 or 21 ms, `/health/live` p50 0 to 1 ms (all
  A runs against all B runs). Likely cause, not isolated by this run: the
  20 users are a closed loop on a CPU-bound 1 vCPU app, so shorter writes let
  each user issue more requests per second and every request queues a little
  longer behind the same CPU. The write latency drop is therefore the
  per-call gain; the read increase is the price of the extra throughput.
- The app is CPU-bound at its cap, which limits how much of the saved wait
  can become throughput. Per-request CPU is 2.5% lower, inside the spread.
- The change stays on correctness as decided; this run neither adds to nor
  subtracts from that case. Bulk `update_many` / `delete_many` were not
  measured.
