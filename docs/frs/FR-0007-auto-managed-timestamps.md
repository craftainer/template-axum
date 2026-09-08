# FR-0007. Manage created_at/updated_at automatically

## Status

Implemented

## Description

The system shall set `created_at` on record creation and refresh
`updated_at` on every create/update, without accepting either as
client-supplied input.

## Source

Standard audit-timestamp convention; see ADR 0001.

## Acceptance criteria

- `HeroCreate`/`HeroUpdate` (`src/views/hero.rs`) have no `created_at`/
  `updated_at` fields -- a client cannot set them even if it tries.
- Both repository implementations set `created_at`/`updated_at` to the
  current UTC time on create, and `updated_at` on every update.
