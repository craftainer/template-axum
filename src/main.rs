//! Entry point: a trivial axum server exposing `/health/live`, proving the
//! toolchain/devcontainer/CI/release foundation works end to end before any
//! real application code (routes, models, business logic) is added in
//! phase 2.

use axum::{routing::get, Router};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

/// Returns 200 to prove the process is up -- phase 2 adds `/health/ready`
/// (checks Postgres/Redis/S3/OIDC) alongside this.
async fn health_live() -> &'static str {
    "ok"
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().json().init();

    let app = Router::new()
        .route("/health/live", get(health_live))
        .layer(TraceLayer::new_for_http());

    let listener = TcpListener::bind("0.0.0.0:8000")
        .await
        .expect("failed to bind 0.0.0.0:8000");
    tracing::info!("listening on {}", listener.local_addr().unwrap());

    axum::serve(listener, app).await.expect("server error");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_live_returns_200() {
        let app = Router::new().route("/health/live", get(health_live));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health/live")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
