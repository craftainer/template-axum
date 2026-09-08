# FR-0026. Allow bulk update and delete of Hero via filters

## Status

Implemented

## Description

The system shall let a caller `PATCH`/`DELETE`
`/crud/v1/heroes/v2/json` with no `?id=` but at least one filter query
parameter to apply the same update (or soft-delete) to every matching
record the caller owns, returning how many records matched and their
ids.

## Source

Port of `controllers/crud_actions.py`'s `resolve_update`/
`resolve_delete` bulk branch. See ADR 0013.

## Acceptance criteria

- `PATCH ?power_level=5` with a JSON body applies that body's set
  fields to every non-archived record owned by the caller whose
  `power_level` is `5`, returning `{"matched": N, "ids": [...]}` (200).
  A different caller's matching records are never touched (ownership
  scoping, ADR 0007/0011).
- `DELETE ?power_level=7` soft-deletes every matching owned record the
  same way, returning the same shape (200) instead of `204` -- there's
  no single record's absence to signal with an empty body for a bulk
  action.
- `PATCH`/`DELETE` with neither `?id=` nor any filter returns `422`
  (`id or at least one filter is required for a bulk action`).
- A bulk action whose filters match more records than
  `Settings::bulk_action_max_matched` (default 1000) returns `422`
  rather than running (`NFR-0027`).
- Verified by `controllers::heroes::tests::
  bulk_update_applies_the_payload_to_every_matching_owned_record`/
  `bulk_update_with_no_id_and_no_filters_returns_422`/
  `bulk_delete_soft_deletes_every_matching_owned_record`.
