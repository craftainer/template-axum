# FR-0024. Provide a reusable Sunset/Deprecation header mechanism

## Status

Implemented

## Description

The system shall provide a reusable way for a route handler to attach
an RFC 8594 `Sunset` header (HTTP-date format), a `Deprecation: true`
header, and an optional `Link: <path>; rel="sunset"` header to its own
response, with no effect on any other route's response.

## Source

Port of the `sunset()` half of `app/http_headers.py`. See ADR 0012 and
`docs/nfrs/NFR-0026-deprecation-sunset-headers.md` (this instance's
port of the reference's `NFR-0002`).

## Acceptance criteria

- `http_headers::Sunset::new(at, link)` combined into a handler's
  return type sets `Deprecation: true` and `Sunset: <RFC 7231
  HTTP-date>` on that response.
- Passing `Some(link)` additionally sets `Link: <link>; rel="sunset"`;
  `None` omits the header entirely.
- A response that doesn't include `Sunset` carries none of these
  headers -- structurally guaranteed here (opt-in per response, not
  middleware), rather than needing its own negative test the way the
  reference's global-middleware design does.
- Verified by `http_headers::tests` (`sets_deprecation_true_and_a_sunset_http_date`,
  `omits_link_when_none`, `sets_link_with_sunset_rel_when_given`).
