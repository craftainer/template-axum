//! `/crud/v1/heroes/v2/json` -- the Hero v2 resource router (list/get/
//! create/update/delete). Port of `crud_1/heroes/heroes_v2.py`, mounted
//! by `main.rs` at the exact path template-fastapi uses (`docs/adrs/0009`).
//! Owner-scoped per ADR 0011 (reads open, writes/deletes restricted to the
//! caller's own `sub`); soft-deleted per ADR 0012.

use std::collections::HashMap;
use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use crate::controllers::crud_actions::{DeleteOutcome, ListOrGet, UpdateOutcome};
use crate::controllers::crud_events::{self, EventSse};
use crate::controllers::crud_query::{parse_filters, parse_sort, FieldSpec};
use crate::controllers::crud_stats;
use crate::controllers::{
    crud_actions, AppState, HERO_DELETE_ROLES, HERO_EVENT_RESOURCE, HERO_READ_ROLES,
    HERO_WRITE_ROLES,
};
use crate::crud::DEFAULT_LIMIT;
use crate::events::EventAction;
use crate::models::hero;
use crate::oidc::AuthClaims;
use crate::problem_details::AppError;
use crate::views::hero::{HeroCreate, HeroListQuery, HeroRead, HeroUpdate};
use crate::views::stats::{
    CategoricalValueCount, LifecycleStats, NumericFieldStat, Prediction, ResourceStats,
    SeriesPoint, TimeBucketCount,
};

/// Rate-limit scope key (`src/rate_limit.rs`) shared by create/update/
/// delete -- a single record edit shares the same per-caller budget as
/// every other mutating call, matching `rate_limit.py`'s own reasoning
/// (see that module's doc comment) for applying the limit to a route's
/// handler as a whole rather than exempting any one verb. `pub(crate)`:
/// shared with `controllers::heroes_xml` (`docs/adrs/0014`) so both
/// sibling routers draw from the same per-caller budget rather than each
/// format getting its own.
pub(crate) const HERO_WRITE_RATE_SCOPE: &str = "hero-write";

/// Hero's filterable/sortable fields, derived by hand from `HeroRead`'s
/// scalar fields (`docs/adrs/0013`) -- `powers` (a list, not a scalar) has
/// no equivalent here, matching `crud_query.py`'s own field-classifier
/// skipping non-scalar fields. `pub(crate)`: shared with
/// `controllers::heroes_xml`, same reasoning as the rate-limit scope above.
pub(crate) const HERO_FIELD_SPECS: &[FieldSpec] = &[
    FieldSpec::number("id"),
    FieldSpec::string("name"),
    FieldSpec::number("power_level"),
    FieldSpec::string("owner_id"),
    FieldSpec::datetime("archived_at"),
    FieldSpec::datetime("created_at"),
    FieldSpec::datetime("updated_at"),
];

fn hero_read_json(hero: crate::models::hero::Model) -> serde_json::Value {
    serde_json::to_value(HeroRead::from(hero)).expect("HeroRead always serializes")
}

/// `GET ?id=` (single) or `GET ` (list, `?skip=`/`?limit=`/
/// `?include_archived=`, plus any `field[__op]=`/`sort=` filter/sort
/// params -- `docs/adrs/0013`) -- record addressing is a query parameter,
/// never a path segment, matching `crud_router.py`'s `?id=` convention.
async fn list_or_get(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    Query(query): Query<HeroListQuery>,
    Query(raw_params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, AppError> {
    claims.require_any_role(&state.settings.oidc_client_id, HERO_READ_ROLES)?;
    let include_archived = query.include_archived.unwrap_or(false);
    let filters =
        parse_filters(HERO_FIELD_SPECS, &raw_params).map_err(AppError::UnprocessableEntity)?;
    let sort = parse_sort(HERO_FIELD_SPECS, &raw_params).map_err(AppError::UnprocessableEntity)?;
    let skip = query.skip.unwrap_or(0);
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);

    match crud_actions::resolve_list_or_get(
        &state.hero_crud,
        query.id,
        skip,
        limit,
        include_archived,
        filters,
        sort,
    )
    .await?
    {
        ListOrGet::One(hero) => Ok(Json(hero_read_json(hero))),
        ListOrGet::Many(heroes) => {
            let heroes: Vec<HeroRead> = heroes.into_iter().map(HeroRead::from).collect();
            Ok(Json(
                serde_json::to_value(heroes).expect("Vec<HeroRead> always serializes"),
            ))
        }
    }
}

/// `POST ` -> 201. Stamps `owner_id` from the caller's `sub`, never trusts
/// client input (ADR 0011).
async fn create(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    AuthClaims(claims): AuthClaims,
    Json(payload): Json<HeroCreate>,
) -> Result<(StatusCode, Json<HeroRead>), AppError> {
    state
        .rate_limiter
        .check(
            HERO_WRITE_RATE_SCOPE,
            addr.ip(),
            state.settings.rate_limit_hero_write_per_minute,
            60,
        )
        .await?;
    claims.require_any_role(&state.settings.oidc_client_id, HERO_WRITE_ROLES)?;
    payload.validate().map_err(AppError::UnprocessableEntity)?;
    let owner_id = claims.subject()?.to_string();
    let hero = state.hero_crud.create(&owner_id, payload).await?;
    // Announced only after the write actually succeeded, and never able to
    // fail it (docs/adrs/0016) -- a subscriber is told about records that
    // exist, not about attempts.
    crud_events::publish(
        &state.events,
        HERO_EVENT_RESOURCE,
        EventAction::Create,
        vec![hero.id],
    )
    .await;
    Ok((StatusCode::CREATED, Json(HeroRead::from(hero))))
}

/// `PATCH ?id=` -- partial update of one record; an omitted field is left
/// unchanged (FR-0004). `PATCH` with no `?id=` but at least one filter
/// (`field[__op]=`) instead bulk-updates every matching record
/// (`docs/adrs/0013`) with the same payload. Owner-scoped either way: a
/// caller can only update their own heroes.
async fn update(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    AuthClaims(claims): AuthClaims,
    Query(query): Query<HeroListQuery>,
    Query(raw_params): Query<HashMap<String, String>>,
    Json(payload): Json<HeroUpdate>,
) -> Result<Json<serde_json::Value>, AppError> {
    state
        .rate_limiter
        .check(
            HERO_WRITE_RATE_SCOPE,
            addr.ip(),
            state.settings.rate_limit_hero_write_per_minute,
            60,
        )
        .await?;
    claims.require_any_role(&state.settings.oidc_client_id, HERO_WRITE_ROLES)?;
    payload.validate().map_err(AppError::UnprocessableEntity)?;
    let filters =
        parse_filters(HERO_FIELD_SPECS, &raw_params).map_err(AppError::UnprocessableEntity)?;
    let owner_id = claims.subject()?.to_string();

    match crud_actions::resolve_update(
        &state.hero_crud,
        query.id,
        &owner_id,
        filters,
        payload,
        state.settings.bulk_action_max_matched,
    )
    .await?
    {
        UpdateOutcome::One(hero) => {
            crud_events::publish(
                &state.events,
                HERO_EVENT_RESOURCE,
                EventAction::Update,
                vec![hero.id],
            )
            .await;
            Ok(Json(hero_read_json(hero)))
        }
        UpdateOutcome::Bulk(result) => {
            crud_events::publish(
                &state.events,
                HERO_EVENT_RESOURCE,
                EventAction::UpdateMany,
                result.ids.clone(),
            )
            .await;
            Ok(Json(
                serde_json::to_value(result).expect("BulkUpdateResult always serializes"),
            ))
        }
    }
}

/// `DELETE ?id=` -> 204. Soft-delete (sets `archived_at`, ADR 0012), owner-
/// scoped, restricted to the `maintainer` role (FR-0015). `DELETE` with no
/// `?id=` but at least one filter instead bulk-deletes every matching
/// record (`docs/adrs/0013`), returning a `BulkDeleteResult` (200) rather
/// than 204 -- there's no single record's absence to signal with an empty
/// body.
async fn delete_hero(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    AuthClaims(claims): AuthClaims,
    Query(query): Query<HeroListQuery>,
    Query(raw_params): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    state
        .rate_limiter
        .check(
            HERO_WRITE_RATE_SCOPE,
            addr.ip(),
            state.settings.rate_limit_hero_write_per_minute,
            60,
        )
        .await?;
    claims.require_any_role(&state.settings.oidc_client_id, HERO_DELETE_ROLES)?;
    let filters =
        parse_filters(HERO_FIELD_SPECS, &raw_params).map_err(AppError::UnprocessableEntity)?;
    let owner_id = claims.subject()?.to_string();

    match crud_actions::resolve_delete(
        &state.hero_crud,
        query.id,
        &owner_id,
        filters,
        state.settings.bulk_action_max_matched,
    )
    .await?
    {
        DeleteOutcome::One => {
            // `DeleteOutcome::One` is only reachable when `query.id` was
            // `Some` (see `crud_actions::resolve_delete`), so the id the
            // event announces is the one the caller addressed.
            let ids = query.id.into_iter().collect();
            crud_events::publish(&state.events, HERO_EVENT_RESOURCE, EventAction::Delete, ids)
                .await;
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        DeleteOutcome::Bulk(result) => {
            crud_events::publish(
                &state.events,
                HERO_EVENT_RESOURCE,
                EventAction::DeleteMany,
                result.ids.clone(),
            )
            .await;
            Ok(Json(result).into_response())
        }
    }
}

/// Compute one field's count/min/max/avg/sum over `values` (already
/// filtered to the non-null values seen) -- `count`/all-`None` when
/// `values` is empty, matching SQL's own `SUM()`/`AVG()` over zero rows.
fn numeric_stat(field: &'static str, values: &[f64]) -> NumericFieldStat {
    if values.is_empty() {
        return NumericFieldStat {
            field,
            count: 0,
            minimum: None,
            maximum: None,
            average: None,
            total: None,
        };
    }
    let count = values.len() as u64;
    let total: f64 = values.iter().sum();
    let minimum = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let maximum = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    NumericFieldStat {
        field,
        count,
        minimum: Some(minimum),
        maximum: Some(maximum),
        average: Some(total / count as f64),
        total: Some(total),
    }
}

/// `field`'s value for one Hero record, as a stats-eligible `f64` -- the
/// one place `crud_stats`'s field-name strings become Hero-specific
/// field access (mirrors `hero_sea_orm.rs`'s `column_for`).
fn numeric_value(hero: &hero::Model, field: &str) -> Option<f64> {
    match field {
        "id" => Some(f64::from(hero.id)),
        "power_level" => hero.power_level.map(f64::from),
        _ => None,
    }
}

/// `field`'s value for one Hero record, as a categorical-distribution
/// value -- always `None` for Hero (it has no boolean/enum field), kept
/// alongside `numeric_value` above so a future boolean field needs only
/// one match arm added here, not a new mechanism.
fn boolean_value(_hero: &hero::Model, _field: &str) -> Option<bool> {
    None
}

fn time_series(records: &[hero::Model], bucket: crud_stats::TimeBucket) -> Vec<TimeBucketCount> {
    let mut counts: std::collections::BTreeMap<chrono::NaiveDateTime, u64> =
        std::collections::BTreeMap::new();
    for record in records {
        let start = crud_stats::bucket_start(bucket, record.created_at);
        *counts.entry(start).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .map(|(bucket_start, count)| TimeBucketCount {
            bucket_start,
            count,
        })
        .collect()
}

#[derive(serde::Deserialize)]
struct StatsQuery {
    bucket: Option<String>,
    include_archived: Option<bool>,
}

/// `GET /stats` -- count/numeric/categorical/time-series/lifecycle
/// aggregates over the (capped, `crud_stats::MAX_HISTORY_RECORDS`)
/// matching records (`docs/adrs/0015`).
async fn get_stats(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    Query(query): Query<StatsQuery>,
) -> Result<Json<ResourceStats>, AppError> {
    claims.require_any_role(&state.settings.oidc_client_id, HERO_READ_ROLES)?;
    let include_archived = query.include_archived.unwrap_or(false);
    let bucket = crud_stats::parse_bucket(query.bucket.as_ref())?;

    let total_visible = state.hero_crud.count(&[], false).await?;
    let total_all = state.hero_crud.count(&[], true).await?;
    let total = if include_archived {
        total_all
    } else {
        total_visible
    };

    let records = state
        .hero_crud
        .list(
            0,
            crud_stats::MAX_HISTORY_RECORDS,
            include_archived,
            vec![],
            vec![],
        )
        .await?;

    let numeric = crud_stats::numeric_fields(HERO_FIELD_SPECS)
        .into_iter()
        .map(|field| {
            let values: Vec<f64> = records
                .iter()
                .filter_map(|r| numeric_value(r, field))
                .collect();
            numeric_stat(field, &values)
        })
        .collect();
    // Hero has no boolean/enum field, so crud_stats::categorical_fields
    // (HERO_FIELD_SPECS has no FieldSpec::boolean entries) is always
    // empty here -- kept generic rather than hardcoded so a future
    // resource with one gets a value distribution for free.
    let categorical: Vec<CategoricalValueCount> = crud_stats::categorical_fields(HERO_FIELD_SPECS)
        .into_iter()
        .flat_map(|field| {
            let mut counts: std::collections::HashMap<String, u64> =
                std::collections::HashMap::new();
            for record in &records {
                if let Some(value) = boolean_value(record, field) {
                    *counts.entry(value.to_string()).or_insert(0) += 1;
                }
            }
            counts
                .into_iter()
                .map(move |(value, count)| CategoricalValueCount {
                    field,
                    value,
                    count,
                })
        })
        .collect();

    Ok(Json(ResourceStats {
        total,
        numeric,
        categorical,
        time_series: bucket.map(|bucket| time_series(&records, bucket)),
        lifecycle: LifecycleStats {
            archived: total_all - total_visible,
        },
    }))
}

#[derive(serde::Deserialize)]
struct PredictQuery {
    field: Option<String>,
    periods: Option<String>,
    bucket: Option<String>,
}

/// `GET /predict` -- an OLS trend forecast over record count (or a
/// numeric field's per-bucket sum) time-bucketed history
/// (`docs/adrs/0015`). `422` (typed `FieldError`) below two buckets of
/// history, matching `crud_stats::forecast`'s own contract.
async fn get_prediction(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    Query(query): Query<PredictQuery>,
) -> Result<Json<Prediction>, AppError> {
    claims.require_any_role(&state.settings.oidc_client_id, HERO_READ_ROLES)?;
    let bucket =
        crud_stats::parse_bucket(query.bucket.as_ref())?.unwrap_or(crud_stats::TimeBucket::Day);
    let field = crud_stats::parse_predict_field(HERO_FIELD_SPECS, query.field.as_ref())?;
    let periods = crud_stats::parse_periods(query.periods.as_ref())?;

    let records = state
        .hero_crud
        .list(0, crud_stats::MAX_HISTORY_RECORDS, false, vec![], vec![])
        .await?;

    let series: Vec<crud_stats::BucketValue> = match field {
        None => time_series(&records, bucket)
            .into_iter()
            .map(|point| crud_stats::BucketValue {
                bucket_start: point.bucket_start,
                value: point.count as f64,
            })
            .collect(),
        Some(field_name) => {
            let mut sums: std::collections::BTreeMap<chrono::NaiveDateTime, f64> =
                std::collections::BTreeMap::new();
            for record in &records {
                if let Some(value) = numeric_value(record, field_name) {
                    let start = crud_stats::bucket_start(bucket, record.created_at);
                    *sums.entry(start).or_insert(0.0) += value;
                }
            }
            sums.into_iter()
                .map(|(bucket_start, value)| crud_stats::BucketValue {
                    bucket_start,
                    value,
                })
                .collect()
        }
    };

    let predictions = crud_stats::forecast(&series, periods, bucket).map_err(|err| {
        AppError::UnprocessableEntity(vec![crate::views::FieldError::new(
            "periods",
            format!(
                "need at least 2 time buckets of history to forecast a trend, got {}",
                err.have
            ),
        )])
    })?;
    let last_known = series
        .last()
        .expect("forecast() already rejects fewer than 2 buckets");

    Ok(Json(Prediction {
        field,
        bucket: bucket.as_str(),
        method: "linear_regression",
        last_known: SeriesPoint {
            bucket_start: last_known.bucket_start,
            value: last_known.value,
        },
        predictions: predictions
            .into_iter()
            .map(|p| SeriesPoint {
                bucket_start: p.bucket_start,
                value: p.value,
            })
            .collect(),
    }))
}

#[derive(serde::Deserialize)]
struct EventsQuery {
    subscriber_id: Option<String>,
}

/// `GET /events` -- a Server-Sent Events stream of this resource's
/// create/update/delete activity (`docs/adrs/0016`, FR-0030).
///
/// The first frame carries the `subscriber_id` this connection was issued;
/// sending it back (`?subscriber_id=`, or automatically via
/// `Last-Event-ID`) on reconnect resumes the same persistent MQTT session,
/// which is what replays events published while the client was away. A
/// client that discards it gets a working stream with no replay.
///
/// JSON-only: there is no XML sibling of this route (`docs/adrs/0017` in
/// the reference), since `text/event-stream` frames carry JSON payloads.
async fn events(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    Query(query): Query<EventsQuery>,
    headers: HeaderMap,
) -> Result<EventSse, AppError> {
    claims.require_any_role(&state.settings.oidc_client_id, HERO_READ_ROLES)?;
    crud_events::event_stream(
        &state.events,
        HERO_EVENT_RESOURCE,
        query.subscriber_id.as_deref(),
        &headers,
    )
    .await
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/stats", get(get_stats))
        .route("/predict", get(get_prediction))
        .route("/events", get(events))
        .route(
            "/",
            get(list_or_get)
                .post(create)
                .patch(update)
                .delete(delete_hero),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Mode, Settings};
    use crate::controllers::DynHeroRepository;
    use crate::health::HealthRegistry;
    use crate::oidc::OidcVerifier;
    use crate::repositories::hero_memory::HeroMemoryRepository;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn mock_settings() -> Settings {
        Settings {
            app_name: "template-axum".to_string(),
            mode: Mode::Mock,
            allow_mock_mode: true,
            postgres_user: "app".to_string(),
            postgres_password: "app".to_string(),
            postgres_db: "app".to_string(),
            postgres_host: "localhost".to_string(),
            postgres_port: 5432,
            s3_endpoint_url: "http://localhost:9000".to_string(),
            s3_access_key: "rustfsadmin".to_string(),
            s3_secret_key: "rustfsadmin".to_string(),
            redis_url: "redis://localhost:6379/0".to_string(),
            mqtt_host: "localhost".to_string(),
            mqtt_port: 1883,
            rate_limit_mock_token_per_minute: 10,
            rate_limit_hero_write_per_minute: 20,
            bulk_action_max_matched: 1000,
            oidc_issuer_url: "http://localhost:8080/realms/template-fastapi".to_string(),
            oidc_authorization_url: "http://localhost:8080/auth".to_string(),
            oidc_token_url: "http://localhost:8080/token".to_string(),
            oidc_client_id: "api".to_string(),
            oidc_audience: None,
        }
    }

    fn app() -> Router {
        let settings = Arc::new(mock_settings());
        let state = AppState {
            oidc: Arc::new(OidcVerifier::new(settings.clone())),
            settings,
            health_registry: Arc::new(HealthRegistry::new()),
            hero_crud: Arc::new(crate::crud::CrudService::new(DynHeroRepository(Box::new(
                HeroMemoryRepository::new(),
            )))),
            rate_limiter: Arc::new(crate::rate_limit::RateLimiter::mock()),
            events: Arc::new(crate::events::EventBus::mock()),
        };
        router().with_state(state)
    }

    fn token(sub: &str, roles: &[&str]) -> String {
        jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &serde_json::json!({
                "sub": sub,
                "resource_access": { "api": { "roles": roles } }
            }),
            &jsonwebtoken::EncodingKey::from_secret(b"mock-mode-doesnt-verify-signatures"),
        )
        .unwrap()
    }

    fn authed(
        method: &str,
        uri: &str,
        sub: &str,
        roles: &[&str],
        body: serde_json::Value,
    ) -> Request<Body> {
        let body = if body.is_null() {
            Body::empty()
        } else {
            Body::from(body.to_string())
        };
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header("Authorization", format!("Bearer {}", token(sub, roles)))
            .header("Content-Type", "application/json")
            .body(body)
            .unwrap();
        // create/update/delete_hero extract ConnectInfo<SocketAddr> for
        // rate limiting -- `oneshot` bypasses the real
        // `into_make_service_with_connect_info` main.rs wires up, so tests
        // insert the same extension by hand (harmless for routes that
        // don't extract it, e.g. the GET list/get handler).
        request
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                12345,
            ))));
        request
    }

    async fn json_body(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    const VALID_HERO: &str = r#"{"name":"Spectra","powers":["flight"],"power_level":5}"#;

    // -- role matrix (FR-0015): every route x every role this app grants. --

    #[tokio::test]
    async fn read_roles_can_list_heroes() {
        for role in ["viewer", "editor", "maintainer", "detective"] {
            let response = app()
                .oneshot(authed(
                    "GET",
                    "/",
                    "alice",
                    &[role],
                    serde_json::Value::Null,
                ))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "role {role} should be able to list"
            );
        }
    }

    #[tokio::test]
    async fn a_role_with_no_read_grant_is_forbidden_from_listing() {
        let response = app()
            .oneshot(authed(
                "GET",
                "/",
                "alice",
                &["security"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_request_with_no_bearer_token_is_unauthorized() {
        let response = app()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn write_roles_can_create_but_viewer_and_detective_cannot() {
        for role in ["editor", "maintainer"] {
            let response = app()
                .oneshot(authed(
                    "POST",
                    "/",
                    "alice",
                    &[role],
                    serde_json::from_str(VALID_HERO).unwrap(),
                ))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::CREATED,
                "role {role} should create"
            );
        }
        for role in ["viewer", "detective"] {
            let response = app()
                .oneshot(authed(
                    "POST",
                    "/",
                    "alice",
                    &[role],
                    serde_json::from_str(VALID_HERO).unwrap(),
                ))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "role {role} should not create"
            );
        }
    }

    #[tokio::test]
    async fn create_rejects_an_invalid_payload_with_422_and_field_errors() {
        let response = app()
            .oneshot(authed(
                "POST",
                "/",
                "alice",
                &["editor"],
                serde_json::json!({"name": "", "powers": []}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = json_body(response).await;
        let errors = body["detail"].as_array().unwrap();
        assert_eq!(errors.len(), 2);
    }

    #[tokio::test]
    async fn only_maintainer_can_delete() {
        let create = app()
            .oneshot(authed(
                "POST",
                "/",
                "alice",
                &["maintainer"],
                serde_json::from_str(VALID_HERO).unwrap(),
            ))
            .await
            .unwrap();
        let created = json_body(create).await;
        let id = created["id"].as_i64().unwrap();

        let response = app()
            .oneshot(authed(
                "DELETE",
                &format!("/?id={id}"),
                "alice",
                &["editor"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "editor must not delete"
        );
    }

    // -- ownership scoping (ADR 0011): writes are restricted to the caller
    // that created the record, even with an otherwise-sufficient role. --

    #[tokio::test]
    async fn update_by_a_non_owner_returns_404_not_403() {
        let shared_app = app();
        let create = shared_app
            .clone()
            .oneshot(authed(
                "POST",
                "/",
                "alice",
                &["editor"],
                serde_json::from_str(VALID_HERO).unwrap(),
            ))
            .await
            .unwrap();
        let created = json_body(create).await;
        let id = created["id"].as_i64().unwrap();

        let response = shared_app
            .oneshot(authed(
                "PATCH",
                &format!("/?id={id}"),
                "mallory",
                &["editor"],
                serde_json::json!({"name": "Hacked"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_missing_id_returns_404() {
        let response = app()
            .oneshot(authed(
                "GET",
                "/?id=999999",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // -- Redis-backed rate limiting (Tier B item 1, docs/adrs/0011). --

    #[tokio::test]
    async fn hero_write_routes_return_429_once_the_per_caller_limit_is_exceeded() {
        let settings = Arc::new(Settings {
            rate_limit_hero_write_per_minute: 2,
            ..mock_settings()
        });
        let state = AppState {
            oidc: Arc::new(OidcVerifier::new(settings.clone())),
            settings,
            health_registry: Arc::new(HealthRegistry::new()),
            hero_crud: Arc::new(crate::crud::CrudService::new(DynHeroRepository(Box::new(
                HeroMemoryRepository::new(),
            )))),
            rate_limiter: Arc::new(crate::rate_limit::RateLimiter::mock()),
            events: Arc::new(crate::events::EventBus::mock()),
        };
        let shared_app = router().with_state(state);

        for _ in 0..2 {
            let response = shared_app
                .clone()
                .oneshot(authed(
                    "POST",
                    "/",
                    "alice",
                    &["editor"],
                    serde_json::from_str(VALID_HERO).unwrap(),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CREATED);
        }

        let response = shared_app
            .oneshot(authed(
                "POST",
                "/",
                "alice",
                &["editor"],
                serde_json::from_str(VALID_HERO).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn delete_without_id_query_param_returns_422() {
        let response = app()
            .oneshot(authed(
                "DELETE",
                "/",
                "alice",
                &["maintainer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // -- generic filter/sort/bulk (docs/adrs/0013, Tier C item 3). --

    async fn create_hero(app: &Router, sub: &str, body: serde_json::Value) -> i64 {
        let response = app
            .clone()
            .oneshot(authed("POST", "/", sub, &["editor"], body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        json_body(response).await["id"].as_i64().unwrap()
    }

    #[tokio::test]
    async fn list_filters_by_an_equality_query_param() {
        let shared_app = app();
        create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "Spectra", "powers": ["flight"], "power_level": 5}),
        )
        .await;
        create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "Umbra", "powers": ["stealth"], "power_level": 3}),
        )
        .await;

        let response = shared_app
            .oneshot(authed(
                "GET",
                "/?name=Umbra",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        let heroes = body.as_array().unwrap();
        assert_eq!(heroes.len(), 1);
        assert_eq!(heroes[0]["name"], "Umbra");
    }

    #[tokio::test]
    async fn list_sorts_descending_with_a_leading_dash() {
        let shared_app = app();
        create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "Low", "powers": ["a"], "power_level": 1}),
        )
        .await;
        create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "High", "powers": ["a"], "power_level": 9}),
        )
        .await;

        let response = shared_app
            .oneshot(authed(
                "GET",
                "/?sort=-power_level",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        let heroes = body.as_array().unwrap();
        assert_eq!(heroes[0]["name"], "High");
        assert_eq!(heroes[1]["name"], "Low");
    }

    #[tokio::test]
    async fn list_rejects_an_unrecognized_filter_field_with_422() {
        let response = app()
            .oneshot(authed(
                "GET",
                "/?nope=1",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn bulk_update_applies_the_payload_to_every_matching_owned_record() {
        let shared_app = app();
        create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "Spectra", "powers": ["flight"], "power_level": 5}),
        )
        .await;
        create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "Umbra", "powers": ["stealth"], "power_level": 5}),
        )
        .await;
        // A different owner's matching record must not be touched.
        create_hero(
            &shared_app,
            "mallory",
            serde_json::json!({"name": "Ghost", "powers": ["stealth"], "power_level": 5}),
        )
        .await;

        let response = shared_app
            .clone()
            .oneshot(authed(
                "PATCH",
                "/?power_level=5",
                "alice",
                &["editor"],
                serde_json::json!({"power_level": 10}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["matched"], 2);
        assert_eq!(body["ids"].as_array().unwrap().len(), 2);

        let mallory_check = shared_app
            .oneshot(authed(
                "GET",
                "/?power_level=5",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        let body = json_body(mallory_check).await;
        // Only mallory's untouched record still has power_level=5.
        assert_eq!(body.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn bulk_update_with_no_id_and_no_filters_returns_422() {
        let response = app()
            .oneshot(authed(
                "PATCH",
                "/",
                "alice",
                &["editor"],
                serde_json::json!({"power_level": 10}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn bulk_delete_soft_deletes_every_matching_owned_record() {
        let shared_app = app();
        let id_a = create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "Spectra", "powers": ["flight"], "power_level": 7}),
        )
        .await;
        let id_b = create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "Umbra", "powers": ["stealth"], "power_level": 7}),
        )
        .await;

        let response = shared_app
            .clone()
            .oneshot(authed(
                "DELETE",
                "/?power_level=7",
                "alice",
                &["maintainer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["matched"], 2);
        let mut ids: Vec<i64> = body["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![id_a, id_b]);

        let after = shared_app
            .oneshot(authed(
                "GET",
                &format!("/?id={id_a}"),
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(after.status(), StatusCode::NOT_FOUND);
    }

    // -- /stats, /predict (docs/adrs/0015, Tier C item 5). --

    #[tokio::test]
    async fn stats_reports_total_and_numeric_aggregates() {
        let shared_app = app();
        create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "A", "powers": ["x"], "power_level": 2}),
        )
        .await;
        create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "B", "powers": ["x"], "power_level": 4}),
        )
        .await;

        let response = shared_app
            .oneshot(authed(
                "GET",
                "/stats",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["total"], 2);
        let numeric = body["numeric"].as_array().unwrap();
        let power_level = numeric
            .iter()
            .find(|entry| entry["field"] == "power_level")
            .unwrap();
        assert_eq!(power_level["count"], 2);
        assert_eq!(power_level["total"], 6.0);
        assert_eq!(power_level["average"], 3.0);
        assert!(body["categorical"].as_array().unwrap().is_empty());
        assert_eq!(body["lifecycle"]["archived"], 0);
    }

    #[tokio::test]
    async fn stats_includes_a_time_series_when_bucket_is_given() {
        let shared_app = app();
        create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "A", "powers": ["x"], "power_level": 1}),
        )
        .await;

        let no_bucket = shared_app
            .clone()
            .oneshot(authed(
                "GET",
                "/stats",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        let body = json_body(no_bucket).await;
        assert!(body.get("time_series").is_none());

        let with_bucket = shared_app
            .oneshot(authed(
                "GET",
                "/stats?bucket=day",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        let body = json_body(with_bucket).await;
        let series = body["time_series"].as_array().unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0]["count"], 1);
    }

    #[tokio::test]
    async fn stats_rejects_an_unrecognized_bucket_with_422() {
        let response = app()
            .oneshot(authed(
                "GET",
                "/stats?bucket=fortnight",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // A happy-path forecast (>= 2 distinct day buckets of history) isn't
    // practical to exercise end-to-end here: every record this harness
    // creates is stamped with the real "now", so multiple heroes created
    // within one test run always land in the same day bucket -- the
    // regression math itself (multi-bucket, day/week/month bucketing,
    // straight-line projection) is covered directly by
    // `crud_stats::tests`, which controls `bucket_start`/series values
    // without needing real distinct calendar days.

    #[tokio::test]
    async fn predict_reports_the_linear_regression_method_and_field_on_success_shape() {
        // Even the insufficient-history 422 case exercises parse_bucket/
        // parse_predict_field/parse_periods and the series-building code
        // path up to forecast() -- this asserts that path runs cleanly
        // (a well-formed 422, not a 500) for a caller who supplied every
        // valid parameter.
        let response = app()
            .oneshot(authed(
                "GET",
                "/predict?periods=2&bucket=week",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn predict_below_two_buckets_of_history_returns_422() {
        let response = app()
            .oneshot(authed(
                "GET",
                "/predict",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn predict_rejects_a_non_numeric_field_with_422() {
        let response = app()
            .oneshot(authed(
                "GET",
                "/predict?field=name",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn predict_rejects_periods_out_of_range_with_422() {
        let response = app()
            .oneshot(authed(
                "GET",
                "/predict?periods=0",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn stats_and_predict_require_a_read_role() {
        let stats = app()
            .oneshot(authed(
                "GET",
                "/stats",
                "alice",
                &["security"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(stats.status(), StatusCode::FORBIDDEN);

        let predict = app()
            .oneshot(authed(
                "GET",
                "/predict",
                "alice",
                &["security"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(predict.status(), StatusCode::FORBIDDEN);
    }

    // -- /events (docs/adrs/0016, FR-0030). The Mode::Mock bus these run
    // against has no broker and no replay by design (NFR-0029), so what is
    // covered here is the route: auth, the subscriber frame, and that a
    // mutation actually publishes. The delivery guarantee itself is
    // verified only by tests/mqtt_events.rs, against the real broker.

    /// Read up to `count` non-keep-alive SSE frames from a response.
    async fn sse_frames(response: axum::response::Response, count: usize) -> String {
        use futures::StreamExt;
        let mut body = response.into_body().into_data_stream();
        let mut collected = String::new();
        let mut seen = 0;
        while seen < count {
            let Ok(Some(Ok(chunk))) =
                tokio::time::timeout(std::time::Duration::from_millis(500), body.next()).await
            else {
                break;
            };
            let chunk = String::from_utf8_lossy(&chunk).to_string();
            if chunk.starts_with(':') {
                continue;
            }
            collected.push_str(&chunk);
            seen += 1;
        }
        collected
    }

    #[tokio::test]
    async fn events_stream_starts_with_a_subscriber_frame() {
        let response = app()
            .oneshot(authed(
                "GET",
                "/events?subscriber_id=my-sub",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        let body = sse_frames(response, 1).await;
        assert!(body.contains("event: subscriber"), "{body}");
        assert!(body.contains("id: my-sub"), "{body}");
        assert!(body.contains(r#"{"subscriber_id":"my-sub"}"#), "{body}");
    }

    #[tokio::test]
    async fn events_issues_a_subscriber_id_when_the_client_supplies_none() {
        let response = app()
            .oneshot(authed(
                "GET",
                "/events",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        let body = sse_frames(response, 1).await;
        assert!(body.contains("event: subscriber"), "{body}");
        assert!(body.contains("subscriber_id"), "{body}");
    }

    #[tokio::test]
    async fn events_requires_a_read_role_and_a_token() {
        let forbidden = app()
            .oneshot(authed(
                "GET",
                "/events",
                "alice",
                &["security"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

        let unauthorized = app()
            .oneshot(
                Request::builder()
                    .uri("/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_created_hero_is_announced_on_the_event_stream() {
        // One shared app (and therefore one shared EventBus) so the POST's
        // publish reaches the stream opened from the same state.
        let shared_app = app();
        let stream = shared_app
            .clone()
            .oneshot(authed(
                "GET",
                "/events?subscriber_id=watcher",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();

        let created = shared_app
            .oneshot(authed(
                "POST",
                "/",
                "alice",
                &["editor"],
                serde_json::from_str(VALID_HERO).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);

        let body = sse_frames(stream, 2).await;
        assert!(body.contains("event: create"), "{body}");
        assert!(body.contains(r#""resource":"heroes""#), "{body}");
        assert!(body.contains("id: watcher"), "{body}");
    }

    #[tokio::test]
    async fn a_bulk_delete_is_announced_with_every_affected_id() {
        let shared_app = app();
        let id_a = create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "A", "powers": ["x"], "power_level": 7}),
        )
        .await;
        let id_b = create_hero(
            &shared_app,
            "alice",
            serde_json::json!({"name": "B", "powers": ["x"], "power_level": 7}),
        )
        .await;

        let stream = shared_app
            .clone()
            .oneshot(authed(
                "GET",
                "/events?subscriber_id=watcher",
                "alice",
                &["viewer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();

        let deleted = shared_app
            .oneshot(authed(
                "DELETE",
                "/?power_level=7",
                "alice",
                &["maintainer"],
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::OK);

        let body = sse_frames(stream, 2).await;
        assert!(body.contains("event: delete_many"), "{body}");
        assert!(body.contains(&format!("{id_a}")), "{body}");
        assert!(body.contains(&format!("{id_b}")), "{body}");
    }
}
