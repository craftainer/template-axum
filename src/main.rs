//! Process entry point -- port of `main.py`. Deliberately thin: every
//! piece of wiring lives in `src/lib.rs` so `tests/`' integration tier
//! builds the same router this binary serves, rather than a CI-only
//! lookalike. See `src/README.md` for the module layering.

use std::future::IntoFuture;
use std::net::SocketAddr;
use std::sync::Arc;

use craftainer_observability::otel::instrument_db;
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
    let shutdown_timeout =
        template_axum::config::graceful_shutdown_timeout().unwrap_or_else(|err| {
            eprintln!("invalid configuration: {err}");
            std::process::exit(1);
        });
    // Checked here so a bad or (in production) missing SUBSCRIBER_SECRET
    // exits 1 before any connection is opened; the value is read again by
    // `build_state_with_db` and never held in `Settings`.
    if let Err(err) = template_axum::config::subscriber_secret(settings.mode) {
        eprintln!("invalid configuration: {err}");
        std::process::exit(1);
    }
    let settings = Arc::new(settings);

    let db = connect_database(&settings).await;
    // Pool gauges (`db.client.connections.*`) on the meter provider
    // `configure_telemetry` set; no-ops when no metrics export is configured.
    if let Some(db) = &db {
        instrument_db(db.get_postgres_connection_pool());
    }
    let state = build_state_with_db(settings, db.clone()).await;
    // Cancelled the moment the shutdown signal fires: ends every open
    // `/events` stream, which would otherwise hold the drain open forever.
    let shutdown = state.shutdown.clone();
    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8000")
        .await
        .expect("failed to bind 0.0.0.0:8000");
    tracing::info!("listening on {}", listener.local_addr().unwrap());

    // `into_make_service_with_connect_info` (rather than plain `app.
    // into_make_service()`) makes the caller's socket address available to
    // handlers via the `ConnectInfo<SocketAddr>` extractor -- src/
    // rate_limit.rs's per-IP check needs it.
    let serve = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown.clone().cancelled_owned())
    .into_future();
    tokio::pin!(serve);

    // `with_graceful_shutdown` waits for in-flight requests with no bound of
    // its own, so the bound is a race: the signal cancels the token (which
    // starts the drain and ends the `/events` streams), then the timeout, if
    // set, runs; whichever finishes first decides. On a timeout `serve` is
    // dropped and `main` carries on to the pool close and the OTel flush
    // below, exactly as after a clean drain (docs/adrs/0027).
    let served = tokio::select! {
        result = &mut serve => result,
        () = async {
            shutdown_signal().await;
            shutdown.cancel();
            match shutdown_timeout {
                Some(limit) => tokio::time::sleep(limit).await,
                None => std::future::pending().await,
            }
        } => {
            tracing::warn!(
                "graceful shutdown timed out after {}s, closing with requests still in flight",
                shutdown_timeout.map_or(0, |limit| limit.as_secs())
            );
            Ok(())
        }
    };

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
