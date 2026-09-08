# FR-0025. Allow filtering and sorting Hero's list route

## Status

Implemented

## Description

The system shall let a caller filter and sort `GET
/crud/v1/heroes/v2/json`'s list results via query parameters derived
mechanically from Hero's own scalar fields: `field=value` (equality),
`field__min=`/`field__max=` (range), `field__in=a,b,c` (membership),
`field__contains=`/`field__icontains=` (string substring match, case-
sensitive/insensitive), and `sort=field,-other_field` (comma-separated,
a leading `-` meaning descending).

## Source

Port of `controllers/crud_query.py`'s query-string vocabulary. See
ADR 0013.

## Acceptance criteria

- `GET ?name=Umbra` returns only records whose `name` equals `Umbra`.
- `GET ?sort=-power_level` returns records ordered by `power_level`
  descending.
- An unrecognized field name, an operator not valid for that field's
  kind, or a value that fails to parse as that field's type returns
  `422` with field errors, rather than being silently ignored.
- `id`/`skip`/`limit`/`sort`/`include_archived` are never treated as
  filter fields even if a resource happens to have a same-named
  filterable field.
- Verified by `controllers::crud_query::tests` (the generic parser, in
  isolation) and `controllers::heroes::tests::
  list_filters_by_an_equality_query_param`/`list_sorts_descending_with_a_leading_dash`/
  `list_rejects_an_unrecognized_filter_field_with_422` (end-to-end
  through the router).
