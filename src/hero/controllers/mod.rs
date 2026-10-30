//! Hero's half of the Controller layer: axum routers for this template's
//! example CRUD resource, plus the concrete `AppState` every router (Hero's
//! own and `generic::controllers`' generic ones) is instantiated with.
//! Highest layer besides `main` itself (see `src/README.md`'s "Layering"
//! section) -- may import from every layer below it, nothing may import
//! from here.
//!
//! `AppState` bundles the app's *only* resource-specific dependency
//! (`hero_crud`) alongside the resource-agnostic ones (`settings`/`oidc`/
//! `health_registry`/`rate_limiter`/`events`) because axum's `State`
//! extractor needs exactly one concrete type per `Router`, and this app
//! has exactly one resource so far. A second resource would add its own
//! `Arc<CrudService<DynXRepository>>` field alongside `hero_crud` here
//! (or, if that grows unwieldy, `AppState` would need to become its own
//! small composition root) -- either way, `generic::controllers`' own
//! routers stay unaffected, since they're generic over `S` and never
//! reference this concrete struct at all (see that package's own module
//! doc for the `HasSettings`/`HasHealthRegistry`/`HasRateLimiter`
//! traits `AppState` implements below alongside `oidc::HasOidcVerifier`).

pub mod heroes;
pub mod heroes_v1;
pub mod heroes_v1_xml;
pub mod heroes_web;
pub mod heroes_xml;

use std::sync::Arc;

use axum::http::HeaderMap;

use crate::config::Settings;
use crate::generic::controllers::{HasRateLimiter, HasSettings};
use crate::hero::models::hero;
use crate::hero::views::hero::{HeroCreate, HeroUpdate};
use crate::oidc::{HasOidcVerifier, OidcVerifier};
use crate::problem_details::AppError;
use crate::rate_limit::RateLimiter;
use craftainer_crud::conditional::{check_if_match, list_etag, record_etag};
use craftainer_crud::controllers::crud_actions::ListOrGet;
use craftainer_crud::controllers::crud_query::parse_include_archived;
use craftainer_crud::crud::CrudService;
use craftainer_crud::events::EventBus;
use craftainer_crud::stats_cache::CachedStats;
use craftainer_observability::health::router::HasHealthRegistry;
use craftainer_observability::health::HealthRegistry;
use tokio_util::sync::CancellationToken;

craftainer_crud::dyn_repository!(
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
    /// Hero's view of the shared `/stats`/`/predict` cache, or an uncached
    /// one when `STATS_CACHE_TTL_SECONDS` is unset (`docs/adrs/0024`).
    pub hero_stats: CachedStats,
    /// CRUD event publish/subscribe backing `GET <prefix>/events`
    /// (`docs/adrs/0016`) -- MQTT-backed, or an in-memory fan-out under
    /// `Mode::Mock`.
    pub events: Arc<EventBus>,
    /// Cancelled when the process starts shutting down (`main.rs`); every
    /// `/events` route ends its stream on it so graceful shutdown is not held
    /// open by an SSE client (`docs/adrs/0027`).
    pub shutdown: CancellationToken,
}

/// The resource segment Hero's events publish under
/// (`crud-events/heroes`), shared by the JSON and XML sibling routers so
/// both announce onto the same topic.
pub const HERO_EVENT_RESOURCE: &str = "heroes";

/// The `ETag` a `GET` of `result` carries (`docs/adrs/0023`): strong over
/// one record, weak over a page. Shared by the four JSON/XML routers, so a
/// row has the same tag whichever representation or version served it.
pub(crate) fn read_etag(result: &ListOrGet<hero::Model>) -> String {
    match result {
        ListOrGet::One(hero) => record_etag(hero),
        ListOrGet::Many(heroes) => list_etag(heroes),
    }
}

/// Honour `If-Match` on a single-record `PATCH`/`DELETE` (`docs/adrs/0023`):
/// `412` when the header is present and does not match the record's current
/// tag. Returns without checking when the header is absent, when `id` is
/// `None` (a bulk write has no one representation to compare) and when the
/// record does not exist (the write's own `404` follows). The record is read
/// with the same `include_archived` visibility the write sees.
pub(crate) async fn enforce_if_match(
    state: &AppState,
    headers: &HeaderMap,
    id: Option<i32>,
    raw_params: &[(String, String)],
) -> Result<(), AppError> {
    let Some(id) = id else {
        return Ok(());
    };
    if !headers.contains_key(axum::http::header::IF_MATCH) {
        return Ok(());
    }
    let include_archived =
        parse_include_archived(raw_params).map_err(AppError::UnprocessableEntity)?;
    if let Some(current) = state.hero_crud.get(id, include_archived).await? {
        check_if_match(headers, &record_etag(&current), "Hero")?;
    }
    Ok(())
}

impl HasOidcVerifier for AppState {
    fn oidc_verifier(&self) -> &OidcVerifier {
        &self.oidc
    }
}

impl HasSettings for AppState {
    fn settings(&self) -> &Settings {
        &self.settings
    }
}

impl HasHealthRegistry for AppState {
    fn health_registry(&self) -> &HealthRegistry {
        &self.health_registry
    }
}

impl HasRateLimiter for AppState {
    fn rate_limiter(&self) -> &RateLimiter {
        &self.rate_limiter
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Mode;
    use crate::hero::repositories::hero_memory::HeroMemoryRepository;

    fn mock_state() -> AppState {
        let settings = Arc::new(Settings {
            app_name: "template-axum".to_string(),
            mode: Mode::Mock,
            allow_mock_mode: true,
            postgres_user: "app".to_string(),
            postgres_password: "app".to_string(),
            postgres_db: "app".to_string(),
            postgres_host: "localhost".to_string(),
            postgres_port: 5432,
            db: Default::default(),
            s3_endpoint_url: "http://localhost:9000".to_string(),
            s3_access_key: "rustfsadmin".to_string(),
            s3_secret_key: "rustfsadmin".to_string(),
            redis_url: "redis://localhost:6379/0".to_string(),
            mqtt_host: "localhost".to_string(),
            mqtt_port: 1883,
            rate_limit_mock_token_per_minute: 10,
            rate_limit_hero_write_per_minute: 20,
            bulk_action_max_matched: 1000,
            oidc_issuer_url: "http://localhost:8080".to_string(),
            oidc_authorization_url: "http://localhost:8080/auth".to_string(),
            oidc_token_url: "http://localhost:8080/token".to_string(),
            oidc_client_id: "api".to_string(),
            oidc_audience: None,
        });
        AppState {
            oidc: Arc::new(OidcVerifier::new(settings.clone())),
            settings,
            health_registry: Arc::new(HealthRegistry::new()),
            hero_crud: Arc::new(CrudService::new(DynHeroRepository(Box::new(
                HeroMemoryRepository::new(),
            )))),
            rate_limiter: Arc::new(RateLimiter::mock()),
            hero_stats: CachedStats::uncached(HERO_EVENT_RESOURCE),
            shutdown: CancellationToken::new(),
            events: Arc::new(EventBus::mock()),
        }
    }

    /// `AppState`'s `HasSettings`/`HasHealthRegistry`/`HasRateLimiter`
    /// impls (alongside `HasOidcVerifier`) are what let `generic::
    /// controllers`' routers (`health`/`audit`/`mock`) mount onto this
    /// concrete state with no dependency on it -- see this module's own
    /// doc comment. Each accessor must return the same field it wraps.
    #[test]
    fn accessor_traits_return_the_matching_field() {
        let state = mock_state();
        assert!(std::ptr::eq(state.settings(), state.settings.as_ref()));
        assert!(std::ptr::eq(
            state.health_registry(),
            state.health_registry.as_ref()
        ));
        assert!(std::ptr::eq(
            state.rate_limiter(),
            state.rate_limiter.as_ref()
        ));
        assert!(std::ptr::eq(state.oidc_verifier(), state.oidc.as_ref()));
    }
}
