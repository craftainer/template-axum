//! Process entry point -- port of `main.py`. Deliberately thin: every
//! piece of wiring lives in `src/lib.rs` so `tests/`' integration tier
//! builds the same router this binary serves, rather than a CI-only
//! lookalike. See `src/README.md` for the module layering.

use std::net::SocketAddr;
use std::sync::Arc;

use craftainer_observability::{
    configure_telemetry, logs::configure_telemetry_without_export, Error as ObservabilityError,
    Telemetry,
};
use template_axum::config::Settings;
use template_axum::{build_router, build_state_with_db, connect_database, problem_details};

/// Installs JSON logging (plus OTLP export when the environment asks for
/// it). An exporter that can't be built isn't fatal: carry on with JSON
/// logs only, and say so. Panics only if a subscriber is already installed.
fn init_logging() -> Telemetry {
    match configure_telemetry() {
        Ok(guard) => guard,
        Err(ObservabilityError::ExporterBuild(reason)) => {
            let guard = configure_telemetry_without_export()
                .expect("no subscriber is installed after a failed exporter build");
            tracing::warn!("OTLP log exporter setup failed, continuing without it: {reason}");
            guard
        }
        Err(err) => panic!("failed to install logging: {err}"),
    }
}

#[tokio::main]
async fn main() {
    // Kept alive to the end of `main`: dropping it shuts the OTLP export
    // down, and the last batch is only flushed by that.
    let logging = init_logging();

    let settings = Settings::from_env().unwrap_or_else(|err| {
        eprintln!("invalid configuration: {err}");
        std::process::exit(1);
    });
    problem_details::configure_detail_redaction(settings.mode);
    let settings = Arc::new(settings);

    let db = connect_database(&settings).await;
    let app = build_router(build_state_with_db(settings, db.clone()).await);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8000")
        .await
        .expect("failed to bind 0.0.0.0:8000");
    tracing::info!("listening on {}", listener.local_addr().unwrap());

    // `into_make_service_with_connect_info` (rather than plain `app.
    // into_make_service()`) makes the caller's socket address available to
    // handlers via the `ConnectInfo<SocketAddr>` extractor -- src/
    // rate_limit.rs's per-IP check needs it.
    let served = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await;

    // Closing any clone closes the shared sqlx pool: waits for in-use
    // connections to be returned, then ends every connection cleanly.
    if let Some(db) = db {
        if let Err(err) = db.close().await {
            tracing::warn!("error closing the database pool: {err}");
        }
    }

    // The server has stopped, so blocking on the final export is fine here;
    // `spawn_blocking` keeps the runtime's worker free regardless. Done
    // before the `expect` below so a server error is exported too.
    let _ = tokio::task::spawn_blocking(move || {
        if let Err(err) = logging.shutdown() {
            eprintln!("flushing the OTLP log export failed: {err}");
        }
    })
    .await;
    served.expect("server error");
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
