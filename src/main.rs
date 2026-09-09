//! Process entry point -- port of `main.py`. Deliberately thin: every
//! piece of wiring lives in `src/lib.rs` so `tests/`' integration tier
//! builds the same router this binary serves, rather than a CI-only
//! lookalike. See `src/README.md` for the module layering.

use std::net::SocketAddr;
use std::sync::Arc;

use template_axum::config::Settings;
use template_axum::{build_router, build_state, problem_details, telemetry};

#[tokio::main]
async fn main() {
    telemetry::configure_logging();

    let settings = Settings::from_env().unwrap_or_else(|err| {
        eprintln!("invalid configuration: {err}");
        std::process::exit(1);
    });
    problem_details::configure_detail_redaction(settings.mode);
    let settings = Arc::new(settings);

    let app = build_router(build_state(settings).await);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8000")
        .await
        .expect("failed to bind 0.0.0.0:8000");
    tracing::info!("listening on {}", listener.local_addr().unwrap());

    // `into_make_service_with_connect_info` (rather than plain `app.
    // into_make_service()`) makes the caller's socket address available to
    // handlers via the `ConnectInfo<SocketAddr>` extractor -- src/
    // rate_limit.rs's per-IP check needs it.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("server error");
}
