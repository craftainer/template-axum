# FR-0030. Stream Hero create/update/delete activity over Server-Sent Events

## Status

Implemented

## Description

The system shall expose `GET /crud/v1/heroes/v2/json/events` as a
`text/event-stream` of Hero mutation events. Each event names the action
(`create`, `update`, `update_many`, `delete`, `delete_many`), the
resource, the record ids it affected, and when it happened.

The stream shall issue every connection a `subscriber_id`. Streams are
transient (ADR 0031): the connection uses a clean session, so a
reconnecting client receives a working stream with a new id and no replay,
whatever `subscriber_id` or `Last-Event-ID` it presents, and a client that
missed events refetches. (Durable sessions with replay, within the broker's
own bounds, remain available on the bus as an opt-in this template does not
enable; ADR 0016 describes that mechanism.) A caller shall hold at most 5
concurrent streams and a subscriber id at most 2; a further stream is
refused with `429`.

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
- Every connection is issued a fresh server-minted `subscriber_id`; one the
  client sends (`?subscriber_id=` or `Last-Event-ID`) is ignored, not an
  error.
- A sixth concurrent stream for one caller (the token's `sub`) is `429`;
  another caller is unaffected, and a closed stream frees its slot.
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
- Verified by the `craftainer-crud` crate's own tests of
  `craftainer_crud::controllers::crud_events` (frame shape,
  subscriber-id precedence) and `controllers::heroes::tests::
  events_stream_starts_with_a_subscriber_frame`/
  `events_issues_a_subscriber_id_when_the_client_supplies_none`/
  `events_requires_a_read_role_and_a_token`/
  `a_created_hero_is_announced_on_the_event_stream`/
  `a_bulk_delete_is_announced_with_every_affected_id` against the
  `Mode::Mock` bus (`events_streams_are_capped_per_caller` for the cap), and
  end-to-end by the `craftainer-crud` crate's
  `mqtt`-feature tests against a real Mosquitto service (the app's own
  `tests/mqtt_event_bus.rs` covers its wiring of the bus).
