//! The Controller layer: axum routers. Highest layer besides `main`
//! itself (see `src/README.md`'s "Layering" section) -- may import from
//! every layer below it, nothing may import from here.

pub mod crud_actions;
pub mod crud_query;
pub mod health;
pub mod heroes;
pub mod heroes_xml;
pub mod mock;

use std::sync::Arc;

use crate::config::Settings;
use crate::crud::CrudService;
use crate::health::HealthRegistry;
use crate::models::hero;
use crate::oidc::{HasOidcVerifier, OidcVerifier};
use crate::rate_limit::RateLimiter;
use crate::views::hero::{HeroCreate, HeroUpdate};

crate::dyn_repository!(
    DynHeroRepository,
    model = hero::Model,
    create = HeroCreate,
    update = HeroUpdate
);

/// Hero's read-role set (FR-0015): `viewer`/`editor`/`maintainer`/
/// `detective` can list/get.
pub const HERO_READ_ROLES: &[&str] = &["viewer", "editor", "maintainer", "detective"];
/// Hero's write-role set: `editor`/`maintainer` can create/update.
pub const HERO_WRITE_ROLES: &[&str] = &["editor", "maintainer"];
/// Hero's delete-role set: `maintainer` alone can delete.
pub const HERO_DELETE_ROLES: &[&str] = &["maintainer"];

/// Shared application state -- one instance built at startup in `main.rs`,
/// cloned (cheaply, via `Arc`) into every request.
#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<Settings>,
    pub oidc: Arc<OidcVerifier>,
    pub health_registry: Arc<HealthRegistry>,
    pub hero_crud: Arc<CrudService<DynHeroRepository>>,
    pub rate_limiter: Arc<RateLimiter>,
}

impl HasOidcVerifier for AppState {
    fn oidc_verifier(&self) -> &OidcVerifier {
        &self.oidc
    }
}
