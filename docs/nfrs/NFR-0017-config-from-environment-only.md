# NFR-0017. Read configuration exclusively from the process environment

## Status

Implemented

## Attribute

Operability / twelve-factor compliance.

## Description

The application shall read every setting from process environment
variables only; it shall never read a `.env` file itself, and shall
fail fast (refuse to start) on invalid production configuration.

## Source

See `docs/TEMPLATE.md`'s "Don't" section; ADR 0005.

## Verification

Automated: `src/config.rs`'s test suite (`defaults_to_dev_mode`,
`mock_mode_requires_allow_mock_mode`,
`production_rejects_default_credentials_and_missing_audience`,
`production_accepts_hardened_config`) exercises every validation branch
via `std::env::set_var`/`Settings::from_env`, run as part of `cargo
test`. No `dotenvy`/`.env`-reading crate is used anywhere in `src/`.
