# generic/controllers/

What stays of the generic half of the Controller layer in the app (see
`src/README.md`'s "Generic vs. Hero-specific split"). The resource-agnostic
helpers (`crud_actions`, `crud_query`, `crud_stats`, `crud_events`) and the
`/health` router are the `craftainer-crud` and `craftainer-observability`
crates since `docs/adrs/0020`; what is left here needs this app's
configuration and authentication:

- `audit.rs` (`GET /audit`) and `mock.rs` (`POST /mock/token`, mounted only
  under `Mode::Mock`). Each is generic over its state type `S`, bound only
  by the small accessor traits this package's `mod.rs` defines
  (`HasSettings`/`HasRateLimiter` — mirrors `oidc::HasOidcVerifier`'s own
  pattern) — never the concrete, Hero-bearing
  `crate::hero::controllers::AppState`. `lib.rs::build_router` instantiates
  each at the concrete state type and merges the result with Hero's own
  routers (and the crate's health router) into one `Router<AppState>`.

Hero's own routers (`heroes.rs` and siblings) live in
`crate::hero::controllers`, not here — see that package's own `README.md`.
A resource's router is the one place allowed to reference both this
package and its own resource-specific types concretely; this package
never references `crate::hero` (or any future resource) at all, enforced
by `.github/scripts/check_layering.py` (see `docs/adrs/0009`).

## Do

- Give a new router here that needs some piece of shared state its own
  narrow `HasX` accessor trait, the same way `HasSettings`/
  `HasRateLimiter` do — never a direct dependency on
  `crate::hero::controllers::AppState`.
- Use a small `FakeState` implementing only the trait(s) a router's own
  tests need (see `audit.rs`/`mock.rs`'s test modules), never a real
  resource's repository/state, even in `#[cfg(test)]` code.

## Don't

- Reference `crate::hero` (or any other resource-specific module) from
  this package, in production code or tests — see
  `docs/nfrs/NFR-0004-generic-crud-excludes-resource-logic.md`.
