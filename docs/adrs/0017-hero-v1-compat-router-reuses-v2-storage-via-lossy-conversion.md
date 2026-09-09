# 0017. Serve the Hero v1 compat router as a lossy view over v2 storage, mounted at `heroes/v1/{format}`

## Status

Accepted

## Context

The remaining parity gap this plan closes (item 1) is the reference's
deprecated `heroes_v1` API: a `superpower: String` field where v2 has
`powers: Vec<String>`. `docs/adrs/0002` already reserved the URL shape
for this (`/crud/v{router_version}/heroes/v{model_version}/{format}`)
but never exercised the `model_version` axis -- only `v2` existed. Two
questions this ADR settles: does v1 get its own table/repository, and
where exactly does it mount.

A second Postgres table (or a second `HeroMemoryRepository`) for v1
would mean every write has to stay in sync across two storage
representations -- a `superpower` change and a `powers` change would
need to agree on the *same* underlying record, which is exactly the
kind of dual-write drift `docs/adrs/0008`/`0012`'s soft-delete design
was careful to avoid elsewhere in this app. The reference's own
`heroes_v1.py` doesn't do this either: v1 and v2 share one table,
differing only in which fields client code sees.

## Decision

- **No new storage.** `controllers::heroes_v1`/`heroes_v1_xml` are
  built from the *same* `state.hero_crud` (`DynHeroRepository`, backed
  by `HeroSeaOrmRepository`/`HeroMemoryRepository`) every v2 router
  uses -- zero new `models`/`repositories`/`migration` code. Conversion
  lives entirely in `views::hero_v1`/`hero_v1_xml`, at the same layer
  `views::hero_xml` already does format conversion (`docs/adrs/0014`):
  `HeroReadV1::from(hero::Model)` takes `powers[0]` as `superpower`
  (lossy on `powers.len() > 1` -- v1 never had a way to represent more
  than one power, so this is the only sound mapping);
  `HeroCreateV1`/`HeroUpdateV1` convert (`impl From`) into the existing
  `HeroCreate`/`HeroUpdate` before reaching `CrudService`, so
  `crud_actions::resolve_*` and every RBAC/ownership/rate-limit/filter
  check downstream of it run completely unmodified. `HeroUpdateV1 ->
  HeroUpdate`'s `powers` field is `v1.superpower.map(|s| vec![s])`: an
  omitted `superpower` produces `None`, which `HeroUpdate`'s existing
  "omitted means unchanged" contract (FR-0004) already honors -- no new
  omitted-vs-null handling needed.
- **Mount at `/crud/v1/heroes/v1/json` and `/crud/v1/heroes/v1/xml`**,
  filling in the `model_version=1` slot `docs/adrs/0002` reserved,
  rather than the plan text's informal `/v1/heroes*` shorthand -- the
  three-segment shape stays uniform across every model version, which
  is the entire point ADR-0002 recorded it as a decision at all.
- **RFC 8594 `Sunset`/`Deprecation`/`Link` on every v1 response**
  (`NFR-0026`), via `http_headers::Sunset` (`docs/adrs/0012`'s
  mechanism, applied here for the first time): `Link` points at the
  matching v2 path (`/crud/v1/heroes/v2/{json,xml}`), and the sunset
  date is a shared `heroes_v1::sunset_at()` function both v1 routers
  call, so JSON and XML sunset together rather than independently.
- **v1 mutations publish onto the same `crud-events/heroes` topic**
  v2/XML already do -- a subscriber watches the underlying records, not
  which compat representation a writer happened to use (the same
  reasoning `docs/adrs/0014`'s Consequences section already gives for
  the JSON/XML split).

## Consequences

Adding v1 took two new view files (a DTO/conversion mirror, same shape
as `hero_xml.rs`'s existing one) and two new controller files that
reuse `crud_actions`/`HERO_FIELD_SPECS`/`HERO_WRITE_RATE_SCOPE`
directly -- no change to `crud/`, `repositories/`, or `models/` at all,
confirming `docs/adrs/0002`'s bet that the two version axes would stay
cheap to fill in independently.

The trade-off is the same one `docs/adrs/0014` already accepted for the
XML mirror: `HeroReadV1`/`HeroReadV1Xml` need a field added by hand if
`HeroRead` ever gains one, and the `superpower = powers[0]` mapping is
a one-way, irreversible lossy view -- a client that round-trips through
v1 silently drops every power past the first. This is the reference's
own documented behavior, not a regression introduced by this port.
