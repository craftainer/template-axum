# 0014. Serve Hero's XML sibling representation via `quick-xml`'s serde support, not a hand-rolled codec

## Status

Accepted

## Context

Tier C item 4 ports the reference's sibling-router decision
(`docs/adrs/0005-multi-format-representations-via-sibling-routers.md`):
`/crud/v1/heroes/v2/xml`, a second representation of the same Hero
resource, sharing the same `CrudService`/repository dependency as the
JSON router (`controllers::heroes`) so business logic (validation,
ownership, RBAC, rate limiting, filter/sort/bulk) never duplicates or
drifts between formats. `xml_codec.py` is the reference's hand-rolled
generic (de)serializer for this, built because Python's stdlib
`xml.etree.ElementTree` needed a `defusedxml`-based parsing layer for
safety, and because Pydantic's `model_dump`/`model_validate` needed an
explicit dict-based bridge to get "flat model <-> flat XML" semantics
FastAPI's own JSON path doesn't need help with.

Neither of those two reasons carries over to Rust:

- **No hand-rolled generic bridge needed.** `quick-xml` (this app's
  pinned `=0.42.0`, `serialize` feature) has its own `serde`
  integration (`quick_xml::se`/`quick_xml::de`) that does what
  `xml_codec.py`'s `to_xml`/`from_xml` do by hand: a struct field
  becomes a child element, a `Vec<T>` field repeats using the field's
  own name, and (unlike a Python dict round-trip through
  `model_dump`) a target field's declared type drives parsing directly
  -- an XML text node like `"5"` deserializes straight into an `i32`
  field, no separate coercion step. Writing a `src/xml_codec.rs`
  wrapper around this would just be redundant ceremony around what
  `quick_xml::se::to_string_with_root`/`quick_xml::de::from_str`
  already give directly; `controllers::heroes_xml` calls them inline.
- **No `defusedxml`-equivalent hardening crate needed.**
  `quick_xml::escape::unescape` (what its parser uses to resolve
  entity references) only resolves the five predefined XML entities
  (`&lt; &gt; &amp; &apos; &quot;`) and numeric character references --
  it never expands a DTD-declared custom entity, and
  `EscapeError::TooManyNestedEntities` is an explicit guard against
  deeply nested entity references. The "billion laughs" attack
  `defusedxml` exists to patch against `xml.etree.ElementTree` relies
  on exactly the custom-entity-expansion capability `quick_xml` doesn't
  implement in the first place -- there's no equivalent gap to patch.

One real gap between the two languages' serde-attribute model:
`xml_codec.py`'s `to_xml` skips a `None`-valued field by walking
`model_dump(mode="json")`'s dict at call time, independent of how that
same model serializes to JSON (which renders `null` explicitly). A
Rust `#[derive(Serialize)]`'s attributes (`#[serde(skip_serializing_if
= ...)]`) are attached to the *type*, not the call site -- reusing
`views::hero::HeroRead` as-is for XML would mean either XML also
renders explicit `null`-equivalents, or adding `skip_serializing_if`
to `HeroRead` and silently changing the JSON router's existing
null-rendering behavior too. Neither is acceptable.

## Decision

- `controllers::heroes_xml` calls `quick_xml::se::to_string_with_root`/
  `quick_xml::de::from_str` directly -- no `xml_codec.rs` wrapper
  module.
- `views::hero_xml::HeroReadXml` is a small, field-for-field mirror of
  `HeroRead` with `#[serde(skip_serializing_if = "Option::is_none")]`
  on every `Option` field, built via `HeroReadXml::from(hero::Model)`
  immediately alongside `HeroRead::from`'s own conversion -- the *only*
  logic in that type is the field copy; no validation, ownership, or
  CRUD behavior lives there. `HeroCreate`/`HeroUpdate` (the *input*
  DTOs) are reused as-is for both routers: `#[serde(default)]`, added
  to their `Option` fields, changes nothing for JSON (an `Option` field
  there already defaults to `None` when absent) but is required for
  `quick_xml`'s deserializer, which -- unlike `serde_json`'s -- doesn't
  implicitly default a missing `Option` field without it.
- `controllers::heroes_xml` reuses `controllers::heroes`'s
  `HERO_FIELD_SPECS` (`docs/adrs/0013`) and `HERO_WRITE_RATE_SCOPE`
  (`docs/adrs/0011`) directly (`pub(crate)`), and calls the exact same
  `controllers::crud_actions::resolve_list_or_get`/`resolve_update`/
  `resolve_delete` functions the JSON router does -- this is the
  concrete mechanism behind "reusing the same CRUD/repository
  dependency, no duplicated business logic" the plan calls for: only
  the request/response (de)serialization differs between the two
  router files.
- A list response wraps as `<heroes><hero>...</hero>...</heroes>` via a
  small local `HeroListXml { hero: Vec<HeroReadXml> }` wrapper struct
  -- `quick_xml`'s serde support has no bare "serialize this `Vec<T>`
  as the document root" mode (a document needs exactly one root
  element), so the wrapper's single field supplies both the repeated-
  element name and something to hang the `to_string_with_root("heroes",
  ...)` call on.
- Errors (malformed XML, a validation failure, RBAC, rate limiting)
  still render as `application/problem+json` via the existing
  `AppError`/`problem_details.rs` machinery, uniformly across both
  routers -- this app's RFC 9457 error handling
  (`docs/adrs/0004-uniform-problem-details-error-responses.md`) was
  never format-specific to begin with, so nothing new was needed here.

## Consequences

Adding the XML sibling took one new controller file
(`heroes_xml.rs`), one small mirror view (`hero_xml.rs`), and a two-
line additive change to the existing input DTOs -- `controllers::
heroes` itself, `crud_query.rs`, and `crud_actions.rs` didn't change at
all, matching the reference's own "adding a format is one extra router
factory call against the same dependency" payoff.

The cost: any future field added to `HeroRead` needs the same field
added to `HeroReadXml` by hand (no macro keeps them in sync) --
`views/hero_xml.rs`'s own module doc flags this explicitly, and the
mirror's minimal purpose (one `From` impl, no business logic of its
own) keeps that sync cost small and easy to spot in review. Unlike the
reference, this app has no `_with_dependency_headers` merge step
(`docs/adrs/0005`'s own documented failure mode) to worry about yet --
`src/http_headers.rs`'s `Sunset` type (Tier B item 2) isn't applied to
any route in this plan, so that risk doesn't exist here until a future
deprecated route needs it on both siblings at once.
