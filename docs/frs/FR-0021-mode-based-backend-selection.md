# FR-0021. Drive backend selection from one startup-time MODE

## Status

Implemented

## Description

The system shall read a single `MODE` (`dev`/`mock`/`production`) once
at startup and use it to select the Hero repository backend, the health
check backend, OIDC verification strictness, and whether `POST
/mock/token` is mounted.

## Source

See ADR 0005.

## Acceptance criteria

- All four behaviors branch on the same `Settings::mode` value computed
  once in `main()`.
- `MODE=mock` requires `ALLOW_MOCK_MODE=1` (`Settings::from_env` refuses
  otherwise) -- verified in `src/config.rs`'s
  `mock_mode_requires_allow_mock_mode` test.
