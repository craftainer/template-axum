//! `rate_limit::RateLimiter`'s real-Redis backend, against the
//! devcontainer stack's own already-running Redis (`REDIS_URL`) -- the
//! `Backend::Mock` half is exercised directly in `src/rate_limit.rs`'s
//! colocated unit tests; only the failure path of `RateLimiter::connect`
//! can be tested without a live service (see that module's own test doc).

use std::net::IpAddr;

use template_axum::health::checks::RedisHealthCheck;
use template_axum::health::HealthCheck;
use template_axum::problem_details::AppError;
use template_axum::rate_limit::RateLimiter;

mod common;

fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379/0".to_string())
}

fn ip() -> IpAddr {
    "127.0.0.1".parse().unwrap()
}

#[tokio::test]
async fn connect_succeeds_against_a_real_redis() {
    RateLimiter::connect(&redis_url())
        .await
        .expect("the devcontainer stack's Redis must be running for the integration tier");
}

#[tokio::test]
async fn redis_backend_allows_up_to_the_limit_then_rejects() {
    let limiter = RateLimiter::connect(&redis_url())
        .await
        .expect("the devcontainer stack's Redis must be running for the integration tier");
    let scope = format!("it-scope-{}", common::unique_suffix());

    for _ in 0..3 {
        limiter.check(&scope, ip(), 3, 60).await.unwrap();
    }
    let err = limiter.check(&scope, ip(), 3, 60).await.unwrap_err();
    assert!(matches!(err, AppError::TooManyRequests(_)));
}

#[tokio::test]
async fn redis_health_check_reports_healthy_against_a_real_redis() {
    let check = RedisHealthCheck::new(redis_url());
    assert_eq!(check.name(), "redis");
    let result = check.check().await;
    assert!(result.healthy, "{:?}", result.detail);
    assert!(result.detail.is_none());
}
