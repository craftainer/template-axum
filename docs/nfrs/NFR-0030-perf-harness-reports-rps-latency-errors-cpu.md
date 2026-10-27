# NFR-0030. The perf harness reports RPS, latency percentiles, error rate and CPU per request under a fixed 1-vCPU cap, as JSON

## Status

Implemented

## Attribute

Performance measurability: the perf tier (NFR-0024) must produce numbers
that two commits can be compared on, not only a pass/fail.

## Description

Every run of the perf tier (`tests/perf`, `docs/adrs/0018`) shall write
one machine-readable JSON summary of the **measured window** (the
warm-up ramp excluded), containing at least:

- requests per second (**RPS, the primary metric** for level-2 A/B
  comparisons), the request, success and error counts, and the error
  rate;
- response-time p50, p95 and p99 in milliseconds;
- the CPU the app container used (seconds, and milliseconds per
  successful request), taken from its cgroup v2 `cpu.stat`, together
  with the CFS throttling counters;
- the run's identity and conditions: scenario, mode, users, warm-up and
  window length, git commit, image tag, and the CPU and memory caps.

Real-stack runs shall run the app container capped at **1 vCPU and 2 GB
of memory** by default (overridable, and recorded in the summary), so the
CPU-per-request and throughput figures mean the same thing on every
host. Field names are stable and carry their unit; a rename, removal or
change of meaning bumps `schema_version`.

Mock-mode runs (the CI workflow) produce the same document; their CPU
block is `null` unless the runner exposes the container's cgroup.

Out of scope: the numbers' absolute values. They are host-dependent;
only runs on the same host, in a quiet window, are compared, and the
spread between repeated runs of an unchanged commit is established
before an A/B effect is believed.

## Source

QA / CI (`../stakeholders.md`). The level-2 A/B decision of
`docs/plans/2026-10-templates-sdks-ui.md` (RPS primary) and ADR 0018's
AX4h addendum, which records how the harness gets there.

## Verification

- Unit tests in `tests/perf/src/cpu.rs` (`cpu.stat` parsing, window
  interpolation, throttling deltas, the sampler) and
  `tests/perf/src/summary.rs` (percentiles, window, field set, nulls),
  run with `cargo test -p template-axum-perf`.
- A short smoke run of `tests/perf/run-real-stack.sh` whenever the
  harness changes, checking the summary's `cpu` block is non-null and
  `errors` is 0.
