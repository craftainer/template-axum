//! `cache::RedisStatsCache` and Hero's cached `/stats` and `/predict`
//! (`docs/adrs/0024`), against the devcontainer stack's own Redis
//! (`REDIS_URL`) and, for one test, its MQTT broker.
//!
//! Every test keeps its keys under a prefix of its own and removes them
//! afterwards, so it neither collides with the others nor leaves entries
//! behind. Writes go through an in-memory event bus (isolated per test)
//! except `invalidation_arrives_through_the_real_mqtt_broker`, which is the
//! only test that publishes on the shared `crud-events/heroes` topic.

use std::sync::Arc;
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use craftainer_crud::crud::CrudService;
use craftainer_crud::events::{CrudEvent, EventAction, EventBus};
use craftainer_crud::stats_cache::{CachedStats, StatsCache};
use craftainer_observability::health::HealthRegistry;
use serde_json::{json, Value};
use template_axum::cache::{build_stats_cache, run_invalidation, RedisStatsCache};
use template_axum::config::Mode;
use template_axum::hero::controllers::{heroes, AppState, DynHeroRepository, HERO_EVENT_RESOURCE};
use template_axum::hero::repositories::hero_memory::HeroMemoryRepository;
use template_axum::oidc::OidcVerifier;
use template_axum::rate_limit::RateLimiter;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

mod common;

const TTL: Duration = Duration::from_secs(30);

fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379/0".to_string())
}

/// A test's own key prefix.
fn prefix() -> String {
    format!("it-stats-cache-{}", common::unique_suffix())
}

async fn connect(prefix: &str, ttl: Duration, version: &str) -> RedisStatsCache {
    RedisStatsCache::connect_with_prefix(&redis_url(), ttl, version, prefix)
        .await
        .expect("the devcontainer stack's Redis must be running for the integration tier")
}

/// Every key under `prefix`, straight from Redis (not through the cache).
async fn keys_under(prefix: &str) -> Vec<String> {
    let client = redis::Client::open(redis_url()).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    redis::cmd("KEYS")
        .arg(format!("{prefix}:*"))
        .query_async(&mut conn)
        .await
        .unwrap()
}

async fn cleanup(prefix: &str) {
    let keys = keys_under(prefix).await;
    if keys.is_empty() {
        return;
    }
    let client = redis::Client::open(redis_url()).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    redis::cmd("UNLINK")
        .arg(&keys)
        .query_async::<usize>(&mut conn)
        .await
        .unwrap();
}

fn state(events: Arc<EventBus>, stats: CachedStats) -> AppState {
    let mut settings = common::integration_settings();
    settings.oidc_client_id = "api".to_string();
    let settings = Arc::new(settings);
    AppState {
        oidc: Arc::new(OidcVerifier::new(settings.clone())),
        settings,
        health_registry: Arc::new(HealthRegistry::new()),
        hero_crud: Arc::new(CrudService::new(DynHeroRepository(Box::new(
            HeroMemoryRepository::new(),
        )))),
        rate_limiter: Arc::new(RateLimiter::mock()),
        hero_stats: stats,
        events,
        shutdown: CancellationToken::new(),
    }
}

fn token(roles: &[&str]) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &json!({"sub": "alice", "resource_access": {"api": {"roles": roles}}}),
        &jsonwebtoken::EncodingKey::from_secret(b"mock-mode-doesnt-verify-signatures"),
    )
    .unwrap()
}

fn request(method: &str, uri: &str, roles: &[&str], body: Option<Value>) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("Authorization", format!("Bearer {}", token(roles)))
        .header("Content-Type", "application/json")
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))));
    request
}

/// `(status, X-Stats-Cache, body)` of one authenticated `GET`.
async fn get(app: &Router, uri: &str) -> (StatusCode, Option<String>, Value) {
    let response = app
        .clone()
        .oneshot(request("GET", uri, &["viewer"], None))
        .await
        .unwrap();
    let status = response.status();
    let header = response
        .headers()
        .get("x-stats-cache")
        .map(|value| value.to_str().unwrap().to_string());
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        header,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn create_hero(app: &Router, power_level: i32) {
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/",
            &["editor"],
            Some(json!({"name": "H", "powers": ["x"], "power_level": power_level})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
}

/// Poll `/stats` until `total == expected`, so a test does not race the
/// asynchronous invalidation task.
async fn wait_for_total(app: &Router, expected: i64) -> Option<String> {
    for _ in 0..100 {
        let (_, header, body) = get(app, "/stats").await;
        if body["total"] == expected {
            return header;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("/stats never reported total {expected}");
}

#[tokio::test]
async fn set_get_round_trip_and_miss() {
    let prefix = prefix();
    let cache = connect(&prefix, TTL, "v1").await;
    assert_eq!(cache.get("heroes", "k").await.unwrap(), None);
    cache.set("heroes", "k", json!({"total": 3})).await.unwrap();
    assert_eq!(
        cache.get("heroes", "k").await.unwrap(),
        Some(json!({"total": 3}))
    );
    assert_eq!(keys_under(&prefix).await, [format!("{prefix}:v1:heroes:k")]);
    cleanup(&prefix).await;
}

#[tokio::test]
async fn entries_expire_after_the_ttl() {
    let prefix = prefix();
    let cache = connect(&prefix, Duration::from_secs(1), "v1").await;
    cache.set("heroes", "k", json!(1)).await.unwrap();
    assert_eq!(cache.get("heroes", "k").await.unwrap(), Some(json!(1)));
    tokio::time::sleep(Duration::from_millis(2200)).await;
    assert_eq!(cache.get("heroes", "k").await.unwrap(), None);
    cleanup(&prefix).await;
}

#[tokio::test]
async fn invalidate_drops_one_resource_across_app_versions() {
    let prefix = prefix();
    let v1 = connect(&prefix, TTL, "v1").await;
    let v2 = connect(&prefix, TTL, "v2").await;
    v1.set("heroes", "k", json!(1)).await.unwrap();
    v2.set("heroes", "k", json!(2)).await.unwrap();
    v1.set("villains", "k", json!(3)).await.unwrap();

    v1.invalidate("heroes").await.unwrap();

    assert_eq!(v1.get("heroes", "k").await.unwrap(), None);
    assert_eq!(v2.get("heroes", "k").await.unwrap(), None);
    assert_eq!(v1.get("villains", "k").await.unwrap(), Some(json!(3)));
    cleanup(&prefix).await;
}

#[tokio::test]
async fn versions_never_read_each_others_entries() {
    let prefix = prefix();
    let v1 = connect(&prefix, TTL, "v1").await;
    let v2 = connect(&prefix, TTL, "v2").await;
    v1.set("heroes", "k", json!(1)).await.unwrap();
    assert_eq!(v2.get("heroes", "k").await.unwrap(), None);
    cleanup(&prefix).await;
}

#[tokio::test]
async fn invalidate_removes_more_keys_than_one_scan_batch() {
    let prefix = prefix();
    let cache = connect(&prefix, TTL, "v1").await;
    for i in 0..1200 {
        cache
            .set("heroes", &format!("k{i}"), json!(i))
            .await
            .unwrap();
    }
    cache.invalidate("heroes").await.unwrap();
    assert!(keys_under(&prefix).await.is_empty());
}

#[tokio::test]
async fn stats_and_predict_hit_after_a_miss_with_one_entry_per_query() {
    let prefix = prefix();
    let cache = Arc::new(connect(&prefix, TTL, "v1").await);
    let stats = CachedStats::new(cache, HERO_EVENT_RESOURCE);
    let app = heroes::router().with_state(state(Arc::new(EventBus::mock()), stats));
    create_hero(&app, 2).await;

    assert_eq!(get(&app, "/stats").await.1.as_deref(), Some("miss"));
    let (status, header, body) = get(&app, "/stats").await;
    assert_eq!((status, header.as_deref()), (StatusCode::OK, Some("hit")));
    assert_eq!(body["total"], 1);
    // A different query is a different entry.
    assert_eq!(
        get(&app, "/stats?bucket=day").await.1.as_deref(),
        Some("miss")
    );
    assert_eq!(
        get(&app, "/stats?bucket=day").await.1.as_deref(),
        Some("hit")
    );
    assert_eq!(
        get(&app, "/stats?include_archived=true").await.1.as_deref(),
        Some("miss")
    );

    // /predict needs two buckets of history; a 422 is not cached.
    let (status, header, _) = get(&app, "/predict").await;
    assert_eq!((status, header), (StatusCode::UNPROCESSABLE_ENTITY, None));
    assert_eq!(keys_under(&prefix).await.len(), 3);
    cleanup(&prefix).await;
}

#[tokio::test]
async fn a_write_event_invalidates_the_cached_stats() {
    let prefix = prefix();
    let cache = Arc::new(connect(&prefix, TTL, "v1").await);
    let stats = CachedStats::new(cache, HERO_EVENT_RESOURCE);
    let events = Arc::new(EventBus::mock());
    let app_state = state(events.clone(), stats.clone());
    let shutdown = app_state.shutdown.clone();
    let task = tokio::spawn(run_invalidation(
        stats,
        events,
        HERO_EVENT_RESOURCE,
        Duration::from_millis(50),
        shutdown.clone(),
    ));
    let app = heroes::router().with_state(app_state);

    assert_eq!(get(&app, "/stats").await.1.as_deref(), Some("miss"));
    assert_eq!(get(&app, "/stats").await.1.as_deref(), Some("hit"));
    create_hero(&app, 5).await;
    // The write publishes an event; the task drops the entry; the next
    // read rebuilds with the new row.
    assert_eq!(wait_for_total(&app, 1).await.as_deref(), Some("miss"));
    assert_eq!(get(&app, "/stats").await.1.as_deref(), Some("hit"));

    shutdown.cancel();
    task.await.unwrap();
    cleanup(&prefix).await;
}

#[tokio::test]
async fn invalidation_arrives_through_the_real_mqtt_broker() {
    let prefix = prefix();
    let cache = Arc::new(connect(&prefix, TTL, "v1").await);
    let stats = CachedStats::new(cache.clone(), HERO_EVENT_RESOURCE);
    let host = std::env::var("MQTT_HOST").unwrap_or_else(|_| "localhost".into());
    let port = std::env::var("MQTT_PORT")
        .ok()
        .and_then(|port| port.parse().ok())
        .unwrap_or(1883);
    let events = Arc::new(EventBus::connect(&host, port));
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(run_invalidation(
        stats,
        events.clone(),
        HERO_EVENT_RESOURCE,
        Duration::from_millis(200),
        shutdown.clone(),
    ));

    // The task first invalidates on subscribe, so give it that, then
    // seed an entry and publish.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    cache.set(HERO_EVENT_RESOURCE, "k", json!(1)).await.unwrap();
    events
        .publish(CrudEvent::new(
            HERO_EVENT_RESOURCE,
            EventAction::Create,
            vec![1],
        ))
        .await;
    let mut dropped = false;
    for _ in 0..100 {
        if cache.get(HERO_EVENT_RESOURCE, "k").await.unwrap().is_none() {
            dropped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(dropped, "the event never invalidated the entry");

    shutdown.cancel();
    task.await.unwrap();
    cleanup(&prefix).await;
}

/// A TCP forwarder to Redis that the test can cut, standing in for a Redis
/// outage (the test must not stop the stack's container).
struct CuttableProxy {
    addr: std::net::SocketAddr,
    cut: CancellationToken,
}

impl CuttableProxy {
    async fn to_redis() -> Self {
        let url = redis::parse_redis_url(&redis_url()).expect("REDIS_URL parses");
        let target = format!(
            "{}:{}",
            url.host_str().unwrap_or("localhost"),
            url.port().unwrap_or(6379)
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let cut = CancellationToken::new();
        let token = cut.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = token.cancelled() => return,
                    accepted = listener.accept() => {
                        let Ok((mut inbound, _)) = accepted else { return };
                        let target = target.clone();
                        let token = token.clone();
                        tokio::spawn(async move {
                            let Ok(mut outbound) = tokio::net::TcpStream::connect(&target).await
                            else {
                                return;
                            };
                            tokio::select! {
                                () = token.cancelled() => {}
                                _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound) => {}
                            }
                            let _ = inbound.shutdown().await;
                            let _ = outbound.shutdown().await;
                        });
                    }
                }
            }
        });
        Self { addr, cut }
    }

    fn url(&self) -> String {
        format!("redis://{}/0", self.addr)
    }

    /// Drop every connection and stop listening: Redis is now unreachable.
    fn cut(&self) {
        self.cut.cancel();
    }
}

#[tokio::test]
async fn a_redis_outage_degrades_to_a_miss_and_never_fails_a_request() {
    let proxy = CuttableProxy::to_redis().await;
    let prefix = prefix();
    let cache = Arc::new(
        RedisStatsCache::connect_with_prefix(&proxy.url(), TTL, "v1", &prefix)
            .await
            .unwrap(),
    );
    let stats = CachedStats::new(cache.clone(), HERO_EVENT_RESOURCE);
    let app = heroes::router().with_state(state(Arc::new(EventBus::mock()), stats));
    create_hero(&app, 2).await;
    assert_eq!(get(&app, "/stats").await.1.as_deref(), Some("miss"));
    assert_eq!(get(&app, "/stats").await.1.as_deref(), Some("hit"));

    proxy.cut();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Every operation degrades instead of erroring ...
    assert_eq!(cache.get("heroes", "k").await.unwrap(), None);
    cache.set("heroes", "k", json!(1)).await.unwrap();
    cache.invalidate("heroes").await.unwrap();
    // ... so the route keeps answering 200, rebuilt from the repository.
    for _ in 0..2 {
        let (status, header, body) = get(&app, "/stats").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(header.as_deref(), Some("miss"));
        assert_eq!(body["total"], 1);
    }
    cleanup(&prefix).await;
}

#[tokio::test]
async fn build_stats_cache_connects_to_redis_outside_mock_mode() {
    let cache = build_stats_cache(Mode::Dev, &redis_url(), Some(TTL))
        .await
        .expect("Redis must be running")
        .expect("a TTL turns the cache on");
    let resource = format!("it-build-{}", common::unique_suffix());
    cache.set(&resource, "k", json!(7)).await.unwrap();
    assert_eq!(cache.get(&resource, "k").await.unwrap(), Some(json!(7)));
    cache.invalidate(&resource).await.unwrap();
    assert_eq!(cache.get(&resource, "k").await.unwrap(), None);
}

#[tokio::test]
async fn the_invalidation_task_retries_while_the_broker_is_unreachable() {
    let stats = CachedStats::uncached(HERO_EVENT_RESOURCE);
    // Nothing listens on port 1: every subscribe fails.
    let events = Arc::new(EventBus::connect("127.0.0.1", 1));
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(run_invalidation(
        stats,
        events,
        HERO_EVENT_RESOURCE,
        Duration::from_millis(50),
        shutdown.clone(),
    ));
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        !task.is_finished(),
        "a failing subscribe is retried, not fatal"
    );
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("cancelling ends the task")
        .unwrap();
}
