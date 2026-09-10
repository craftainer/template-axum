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
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("server error");
}

/// Resolves on Ctrl-C or `SIGTERM`, whichever comes first -- letting
/// `axum::serve` drain in-flight requests and `main` return normally
/// (rather than being killed) is also what flushes the LLVM coverage
/// profile when this binary is exercised from `tests/e2e.rs`.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install the Ctrl-C signal handler");
    };
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install the SIGTERM signal handler")
            .recv()
            .await;
    };
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("shutdown signal received, draining in-flight requests");
}
