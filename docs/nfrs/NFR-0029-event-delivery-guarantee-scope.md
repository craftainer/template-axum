# NFR-0029. The event stream's no-missed-events guarantee is scoped to same-subscriber reconnects

## Status

Implemented

## Attribute

Reliability (delivery guarantee), and the honesty of its stated scope.

## Description

A subscriber to `GET /crud/v1/heroes/v2/json/events` (FR-0030) that
disconnects and reconnects **presenting the same `subscriber_id`** shall
receive every event published during its absence, provided it reconnects
within 1 hour (`persistent_client_expiration`) and no more than 1000
events queued in the meantime (`max_queued_messages`).

The guarantee shall not be claimed, in code comments, documentation or
API responses, for any wider case. Specifically:

- A client that reconnects **without** its `subscriber_id` starts a new
  session and receives no replay. This is silent by design — there is no
  backlog to serve it from and no error to raise.
- Under `MODE=mock` there is no broker, no persistent session and no
  replay at all; a subscriber that falls behind the in-memory fan-out
  buffer drops events.
- Beyond the broker's queue bounds above, events are dropped by the
  broker.

## Source

ADR 0016 (MQTT persistent sessions as the mechanism) and the reference
implementation's own equivalent note. Driven by the requirement that a
subscriber not silently miss events across a brief disconnect, and by the
equal requirement that a bounded guarantee not read like an unbounded
one.

## Verification

- `tests/mqtt_events.rs`, against the real Mosquitto service, drops a
  subscriber's connection, publishes while it is away, reconnects with
  the same `subscriber_id`, and asserts the missed events arrive — and,
  as the negative case, that reconnecting with a *fresh* id delivers
  nothing. This is the only test that can verify this NFR: the
  `Mode::Mock` bus has none of the mechanism.
- The scope limits above are restated at each place a reader could
  mistake them: `events`' module doc, `EventBus::subscribe`,
  `crud_events::resolve_subscriber_id`, and FR-0030.
- Re-checked whenever `.devcontainer/stack/mqtt/mosquitto.conf`'s
  `persistent_client_expiration`/`max_queued_messages` change, since the
  numbers above are that file's values, not independent ones.
