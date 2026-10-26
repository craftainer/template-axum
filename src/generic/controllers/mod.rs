//! The generic Controller layer that stays in the app: the routers that
//! need this app's configuration and authentication (`audit`, `mock`),
//! generic over their state type `S` and bound only by the small accessor
//! traits below (mirrors `oidc::HasOidcVerifier`'s own pattern) -- never the
//! concrete, Hero-bearing `crate::hero::controllers::AppState`. The CRUD
//! decision logic and the `/health` router come from the `craftainer-crud`
//! and `craftainer-observability` crates (docs/adrs/0020). `lib.rs::
//! build_router` instantiates each router at the concrete state type and
//! merges it with Hero's own routers into one `Router<AppState>`.

pub mod audit;
pub mod mock;

use crate::config::Settings;
use crate::rate_limit::RateLimiter;

/// Shared state `audit`/`mock` need for `Settings` access (e.g.
/// `oidc_client_id`).
pub trait HasSettings {
    fn settings(&self) -> &Settings;
}

/// Shared state `mock`'s token-minting handler needs.
pub trait HasRateLimiter {
    fn rate_limiter(&self) -> &RateLimiter;
}
