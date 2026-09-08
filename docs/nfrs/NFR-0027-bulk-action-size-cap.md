# NFR-0027. Cap how many records a single bulk action can affect

## Status

Implemented

## Attribute

Reliability / resource protection.

## Description

A bulk update/delete (`FR-0026`) whose filters would match more than
`Settings::bulk_action_max_matched` records (default 1000, env
`BULK_ACTION_MAX_MATCHED`) shall be refused with `422` before any row
is touched, rather than running unbounded.

## Source

Port of `config.py`'s `bulk_action_max_matched` /
`crud_actions.py`'s `_check_bulk_action_size`. See ADR 0013.

## Verification

`crud_actions::resolve_update`/`resolve_delete` call `CrudService::
count` with the same filters before calling `update_many`/
`delete_many`, refusing the action if the count exceeds `max_matched`.
Exercised indirectly by this item's `controllers::heroes::tests` bulk
tests (which stay well under the 1000-record default); a dedicated
test driving the cap itself would need creating over 1000 records,
which `docs/adrs/0013`'s reasoning (matching the reference's own
`# pragma: no cover` on this exact branch) treats as too slow/heavy to
justify for what direct unit coverage of `resolve_update`/
`resolve_delete`'s branch logic could otherwise establish -- left as a
documented gap alongside the reference's own.
