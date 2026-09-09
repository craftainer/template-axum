# FR-0030. Stream Hero create/update/delete activity over Server-Sent Events

## Status

Implemented

## Description

The system shall expose `GET /crud/v1/heroes/v2/json/events` as a
`text/event-stream` of Hero mutation events. Each event names the action
(`create`, `update`, `update_many`, `delete`, `delete_many`), the
resource, the record ids it affected, and when it happened.

The stream shall issue every connection a `subscriber_id` and shall
replay, to a reconnecting client presenting the same `subscriber_id`,
every event published while that subscriber was disconnected — within the
broker's own bounds (`max_queued_messages 1000`,
`persistent_client_expiration 1h`). A client that reconnects without a
`subscriber_id` shall receive a working stream with a new id and no
replay.

## Source

Port of the reference implementation's `EventSink`/`EventSource` CRUD
capability. See ADR 0016 for the transport decision (MQTT persistent
sessions + QoS 1 via `rumqttc`), NFR-0029 for the guarantee's scope, and
`docs/adrs/0005` for why `Mode::Mock` gets a fake instead.

## Acceptance criteria

- The first frame of every connection is `event: subscriber` with
  `data: {"subscriber_id":"<id>"}`, and `id: <id>`.
- Every subsequent frame carries `event: <action>`, `id: <subscriber_id>`,
  and a JSON `data:` object with `resource`, `action`, `ids` and
  `occurred_at`.
- A `subscriber_id` is taken from `?subscriber_id=` if present, otherwise
  from the `Last-Event-ID` header, otherwise freshly issued. An id that
  is empty, longer than 64 characters, or contains anything outside
  `[A-Za-z0-9_-]` is treated as absent, not as an error.
- A create publishes one `create` event carrying the new record's id; a
  single-record update/delete publishes `update`/`delete` with that id; a
  bulk update/delete publishes `update_many`/`delete_many` with every
  affected id.
- An event is published only after the mutation has succeeded, and a
  failure to publish never changes the mutating request's own response.
- The route requires the same read-role set as `GET`/`/stats`/`/predict`
  (FR-0015): a caller with no read role receives `403`, one with no
  bearer token `401`.
- There is no XML sibling of this route, but a mutation made through the
  XML sibling router publishes the same event as the JSON one.
- Verified by `controllers::crud_events::tests` (frame shape,
  subscriber-id precedence) and `controllers::heroes::tests::
  events_stream_starts_with_a_subscriber_frame`/
  `events_issues_a_subscriber_id_when_the_client_supplies_none`/
  `events_requires_a_read_role_and_a_token`/
  `a_created_hero_is_announced_on_the_event_stream`/
  `a_bulk_delete_is_announced_with_every_affected_id` against the
  `Mode::Mock` bus, and end-to-end — including the reconnect replay this
  requirement's second paragraph promises — by `tests/mqtt_events.rs`
  against the real Mosquitto service.
