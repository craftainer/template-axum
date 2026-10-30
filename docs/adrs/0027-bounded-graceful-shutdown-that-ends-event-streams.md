# 0027. Bound graceful shutdown with `GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS`; end `/events` streams with a cancellation token

## Status

Accepted

## Context

`main.rs` already handed `axum::serve` a shutdown signal
(`with_graceful_shutdown`, SIGINT or SIGTERM), closed the database pool
after the server stopped and flushed OTLP export through the `Telemetry`
guard (`docs/adrs/0006`, `0022`). Two gaps remained:

- `with_graceful_shutdown` waits for every open connection with no bound of
  its own. A `GET <prefix>/events` stream (`docs/adrs/0016`) never ends on
  its own, so one connected client kept a terminating pod open until the
  orchestrator sent SIGKILL, and then the pool close and the OTLP flush
  never ran.
- A request that never finishes had the same effect.

template-fastapi has the same problem and the same setting. It passes
`GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS` to uvicorn's
`--timeout-graceful-shutdown` when the variable is set, and passes nothing
when it is unset (`scripts/runner.sh:16-17`). It documents the setting as
"unset (server default)", bounding "how long a scale-down waits for open
connections (SSE streams never end on their own)", with 10 s as the measured
value (`docs/architecture.md:157`). It has no validation beyond uvicorn's own
parsing.

`craftainer-crud` 0.3.0 (its `docs/adrs/0009`) provides the other half:
`controllers::crud_events::event_stream_until(.., CancellationToken)` ends
the stream once the token is cancelled, and a call made after cancellation
subscribes to nothing and returns an empty, complete body (so a client
arriving during the drain gets neither a 429 nor a 503). The library does not
add `EventBus::close()`; the host owns the token.

## Decision

- `AppState.shutdown` is a `tokio_util::sync::CancellationToken`.
  `main.rs` cancels it when the shutdown signal fires; the Hero `/events`
  route (the only `/events` route: the XML and v1 siblings have none) calls
  `event_stream_until` with a clone. `tokio-util` becomes a direct
  dependency at the version already in the tree (`default-features = false`;
  `CancellationToken` needs no feature).
- `config::graceful_shutdown_timeout()` reads
  `GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS`: **unset by default**, as in
  fastapi, meaning no bound (axum's own behaviour); a whole number of
  seconds >= 1 otherwise. Zero, negative, fractional or non-numeric values
  exit 1 at startup with `invalid configuration: ...`, like every other
  setting here (fastapi leaves bad values to uvicorn). It is read next to,
  not inside, `Settings`: no handler reads it, and a new `Settings` field
  would touch every struct literal of it.
- `main.rs` races `axum::serve(..).with_graceful_shutdown(token)` against
  "signal, cancel the token, then sleep the timeout". The signal starts the
  drain and ends the event streams at the same moment. Accepted requests get
  up to the timeout. If it elapses first, the server future is dropped, a
  `graceful shutdown timed out` warning is logged and `main` continues,
  exiting 0, through the same pool close and `Telemetry::shutdown` as after a
  clean drain.
- The setting is passed through by the environment alone: `runner.sh` execs
  the binary, which inherits it, so `Dockerfile` and `runner.sh` do not
  change. fastapi's Granian mapping has no analogue.

## Consequences

- A terminating process with SSE clients now drains in the time its real
  requests need, not the orchestrator's grace period. Operators should set
  the variable below `terminationGracePeriodSeconds` (fastapi measures 10 s)
  so the pool close and the OTLP flush are reached.
- Streams end with a normal close, so an `EventSource` client reconnects
  with `Last-Event-ID` (the persistent MQTT session replays what it missed)
  and meets the next pod; a reconnect that lands on the draining pod gets an
  empty 200 and retries.
- On the timeout path, requests still running are abandoned, not cancelled:
  their tasks end with the runtime. The pool close waits for connections
  those tasks still hold, so a statement stuck in Postgres can still delay
  exit past the timeout (bounded by the database, not by this setting). Left
  as is; a hard bound on `close` would be a new decision.
- With the variable unset, a stuck request still blocks exit, as in
  fastapi. The default is a deliberate parity choice, not a recommendation.
- `tests/main_failure_modes.rs` covers both paths against the real binary: a
  SIGTERM with an open stream and a half-sent request ends the stream, lets
  the request finish with a 201 and exits 0; a request that outlives a 1 s
  timeout exits 0 after the timeout with the warning logged.
