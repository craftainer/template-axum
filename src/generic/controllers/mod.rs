//! The generic Controller layer: resource-agnostic router-building
//! blocks, plus the routers that need nothing resource-specific at all
//! (`health`, `audit`, `mock`). Each router here is generic over its
//! state type `S`, bound only by the small accessor traits below (mirrors
//! `oidc::HasOidcVerifier`'s own pattern) -- never the concrete,
//! Hero-bearing `crate::hero::controllers::AppState` -- so a second
//! resource could reuse every one of these unchanged. `lib.rs::
//! build_router` instantiates each generic router at the concrete state
//! type and merges it with Hero's own routers (`crate::hero::
//! controllers`) into one `Router<AppState>`.

pub mod audit;
pub mod crud_actions;
pub mod crud_events;
pub mod crud_query;
pub mod crud_stats;
pub mod health;
pub mod mock;

use crate::config::Settings;
use crate::health::HealthRegistry;
use crate::rate_limit::RateLimiter;

/// Shared state `audit`/`mock` need for `Settings` access (e.g.
/// `oidc_client_id`).
pub trait HasSettings {
    fn settings(&self) -> &Settings;
}

/// Shared state `health`'s `/ready` handler needs.
pub trait HasHealthRegistry {
    fn health_registry(&self) -> &HealthRegistry;
}

/// Shared state `mock`'s token-minting handler needs.
pub trait HasRateLimiter {
    fn rate_limiter(&self) -> &RateLimiter;
}
