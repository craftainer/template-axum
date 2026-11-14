# 0016. Back the CRUD event stream with MQTT persistent sessions via rumqttc

## Status

Accepted

## Context

Hero's generic CRUD interface (`docs/adrs/0001`) offered no way for a
client to watch a resource's create/update/delete activity in real time —
the only option was polling `GET /crud/v1/heroes/v2/json` on an interval.
The requested shape was `GET <prefix>/events`, a Server-Sent Events
stream, as an opt-in capability of the generic interface.

SSE on its own carries no delivery guarantee. Its only recovery mechanism
is the `Last-Event-ID` header a client *may* resend after a dropped
connection, and a server that has kept no history has nothing to answer
it with. The hard requirement here is that a subscriber must not silently
miss events published while it was briefly disconnected (a backgrounded
tab, a sleeping laptop, a network blip), which rules out anything
fire-and-forget on the publish side.

Three transports were considered:

- **Redis pub/sub**, on the Redis/Valkey service this app already runs
  for rate limiting (`docs/adrs/0011`). Rejected: pub/sub is
  fire-and-forget with no replay, which is exactly the guarantee needed.
- **Redis Streams**, on the same service. Would keep every transport on
  one already-running image, but per-subscriber delivery state (consumer
  groups, `XACK`, pending-entries lists) would have to be driven from
  application code to get anything like a durable per-subscriber queue —
  reimplementing, less well, what a broker already does.
- **Database polling** for recently-changed rows. Rejected on latency
  (real time means sub-second, not poll-interval) and on load: every open
  SSE connection would need its own polling loop against Postgres.

On the Rust client side the open question (carried unanswered from the
plan this work continues) was `rumqttc` vs. the `paho-mqtt` C bindings,
specifically whether persistent sessions and QoS 1 are reachable cleanly
from async Rust.

Two further forces are specific to this instance:

- `Mode::Mock` must keep working with zero containers (`NFR-0022`), so
  whatever transport is chosen needs a local stand-in.
- Cargo's integration-test convention links a test binary against the
  crate's *public API*, and this package was a bin-only crate — so there
  was no library target for a test to import, and no way to test the
  event stream (or anything else) against real services from `tests/`.

## Decision

**We will use MQTT** — the Eclipse Mosquitto service already defined at
`.devcontainer/stack/mqtt/` — with the delivery guarantee built entirely
from MQTT's own persistent-session mechanism rather than from anything
SSE provides:

- Each subscriber is issued a `subscriber_id` (128 bits from the OS
  CSPRNG, hex-encoded) on first connect, delivered as the stream's first
  frame and repeated as the `id:` of every subsequent frame.
- The client passes it back on reconnect as `?subscriber_id=` or — with
  no client-side bookkeeping at all, since it is every frame's `id:` —
  automatically via `Last-Event-ID`.
- The server maps that id to a **persistent MQTT session**:
  `set_clean_session(false)` with the client id `crud-events-<subscriber_id>`
  (`events::client_id_for`), subscribed at **QoS 1** to
  `crud-events/<resource>`. Because the session is persistent, the broker
  queues messages published while that client id is disconnected and
  delivers them when the same client id reconnects. That, and nothing
  else, is what makes "briefly offline, no missed events" hold.
- The queue is bounded by the broker, not by us:
  `.devcontainer/stack/mqtt/mosquitto.conf` sets
  `max_queued_messages 1000` and `persistent_client_expiration 1h`. This
  protects a subscriber that is gone for a bounded window, not forever.
- The guarantee is scoped strictly to **QoS-1, persistent-session
  reconnects with the same `subscriber_id`**. A client that discards its
  id and reconnects fresh gets a new session and no replay, with no error
  raised anywhere — stated plainly in FR-0030 and in `events`' own module
  doc so it is never mistaken for an unbounded one.

**Client library: `rumqttc`** (`=0.25.1`, Apache-2.0), not `paho-mqtt`.
The open question is answered: rumqttc's `AsyncClient`/`EventLoop` pair
exposes `MqttOptions::set_clean_session(bool)` and `QoS::AtLeastOnce`
directly, is pure Rust with no C toolchain or `bindgen` step, and is
tokio-native — where `paho-mqtt` wraps the Eclipse C library and its
callback model. One caveat drove a design detail: `set_clean_session(false)`
**panics on an empty client id**, so every subscriber id is either freshly
generated or run through `events::sanitize_subscriber_id` (non-empty,
≤64 chars, `[A-Za-z0-9_-]` only) before it is interpolated into a client
id. That validation is also what keeps a client-supplied id from carrying
MQTT topic or wildcard characters.

`default-features = false`: rumqttc's default `use-rustls` feature pulls
in a second rustls/tokio-rustls copy for broker TLS this app does not use
(the broker is an in-network plaintext `:1883` stack service), and a
single rustls copy in the graph is the same constraint `aws-sdk-s3` and
`reqwest` are already configured for.

**Publishing is inline in the controller, not a `CrudService` hook.**
`controllers::heroes` calls `crud_events::publish` after a mutation has
already succeeded — the same shape `rate_limit`'s `check` takes at the
top of a handler, and for the same reason: it keeps `crud::CrudService`
free of infrastructure concerns (`NFR-0004`). Publishing can never fail
the caller's request; `EventBus::publish` logs and moves on, because a
write that succeeded must not be reported as failed just because a
side-channel notification could not be delivered.

Both sibling routers publish onto the same `crud-events/heroes` topic:
a subscriber watches the records, not the representation a writer used.
Only the JSON router *serves* the stream.

**Event scope**: `create`, `update`, `update_many`, `delete`,
`delete_many` — every mutating action this app has. (The reference
implementation this port follows also fires on `restore`/`restore_many`;
this instance has no restore route to fire from, so those actions do not
exist here rather than being deliberately excluded.)

**`Mode::Mock`**: `EventBus::mock`, a `tokio::sync::broadcast` fan-out
built once and shared, selected by the same `Mode` branch in
`build_event_bus` that `HeroMemoryRepository` and `RateLimiter::mock` are
selected by. This path is explicitly best-effort: no broker, no
persistent session, no replay, and a subscriber that falls behind the
channel buffer drops events rather than blocking publishers. A test run
against `Mode::Mock` therefore cannot validate the delivery guarantee —
only `tests/mqtt_events.rs`, against the real broker, does.

**JSON only.** There is no `/events` on the XML sibling router
(`docs/adrs/0014`): `text/event-stream` frames carry JSON payloads, and a
second serialization of the same stream would buy nothing.

**We will also split this crate into a library plus a thin binary.**
`src/lib.rs` holds every module and the router/state wiring
(`build_state`/`build_router`/`build_event_bus`); `src/main.rs` only
calls them. This is what gives `tests/` a public API to link against, so
the integration tier exercises the *same* router the binary serves rather
than a CI-only lookalike.

## Consequences

A second resource gets `GET <prefix>/events` by adding one route plus a
resource-name constant — `controllers::crud_events` is entirely
resource-agnostic, the same shape as `crud_actions`/`crud_query`.

The costs are real:

- A fourth backing service is now on the request path for a route.
  `Mode::Mock` still needs none, but `dev`/`production` do, and an
  unreachable broker means `/events` degrades (the stream ends after
  `MAX_CONSECUTIVE_POLL_ERRORS` reconnect attempts and the client must
  reconnect) rather than erroring loudly.
- Each open SSE connection is its own MQTT connection with its own client
  id, since a persistent session is per-client-id by definition. Broker
  connection count now scales with concurrent subscribers, and every
  session that goes away holds broker-side queue state for up to an hour.
- The guarantee's narrowness is a documentation burden that will not go
  away: a naively-written client that reconnects without its
  `subscriber_id` loses events and is told nothing.
- Two new direct dependencies (`rumqttc`, `getrandom`) and a
  `[lib]`/`[[bin]]` split that changes every path in `main.rs` from
  `crate::` to `template_axum::`.
