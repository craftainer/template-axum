# FR-0003. Validate Hero fields on create and update

## Status

Implemented

## Description

The system shall reject a Hero create/update request where `name` is not
1-200 characters, or `powers` is not a list of at least one string with
each element 1-200 characters, with a 422 problem-details response naming
the offending field(s).

## Source

Mirrors template-fastapi's own Hero v2 validation rules; see ADR 0004.

## Acceptance criteria

- `HeroCreate::validate`/`HeroUpdate::validate` (`src/views/hero.rs`)
  return one `FieldError` per violation.
- An empty `powers` list, or a `powers` entry over 200 characters, is
  rejected with a field-scoped message.
- A validation failure renders as `AppError::UnprocessableEntity` -> 422
  `application/problem+json` (ADR 0004).
