//! OTLP log export, end to end, through the real binary (docs/adrs/0020,
//! FR-0035): `main.rs` installs `craftainer-observability`'s logging, a
//! record logged at startup must reach an OTLP/HTTP collector, and the last
//! batch must be flushed on shutdown.
//!
//! The devcontainer stack has no OTLP collector, so an in-process receiver
//! (an axum route that records every POST) stands in for one. Before the
//! switch the app's own exporter used the async `reqwest` client on the batch
//! processor's plain thread and exported nothing, silently; this test is the
//! guard for that.
//!
//! Binds the fixed `0.0.0.0:8000` `main.rs` always uses, so it runs in its
//! own binary (cargo runs test binaries one after another) with its two
//! tests serialized.

use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::http::{HeaderMap, Uri};
use axum::routing::any;
use axum::Router;

static PORT_8000_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One request the stand-in collector received.
struct Received {
    path: String,
    content_type: Option<String>,
    body: Vec<u8>,
}

/// An OTLP/HTTP receiver on an ephemeral port; returns its base URL and the
/// shared list of what it got.
async fn collector() -> (String, Arc<Mutex<Vec<Received>>>) {
    let received = Arc::new(Mutex::new(Vec::new()));
    let sink = received.clone();
    let app = Router::new().fallback(any(move |uri: Uri, headers: HeaderMap, body: Bytes| {
        let sink = sink.clone();
        async move {
            sink.lock().unwrap().push(Received {
                path: uri.path().to_string(),
                content_type: headers
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string),
                body: body.to_vec(),
            });
            // An empty ExportLogsServiceResponse: success, nothing rejected.
            ""
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, received)
}

fn spawn_app(otlp_endpoint: &str, extra_env: &[(&str, &str)], stdout: Stdio) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_template-axum"));
    command
        .env("MODE", "mock")
        .env("ALLOW_MOCK_MODE", "1")
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", otlp_endpoint)
        .stdout(stdout)
        .stderr(Stdio::inherit());
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command
        .spawn()
        .expect("failed to spawn the template-axum binary")
}

async fn wait_until_ready() {
    let client = reqwest::Client::new();
    for _ in 0..60 {
        if let Ok(response) = client.get("http://127.0.0.1:8000/health/live").send().await {
            if response.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("template-axum did not become ready within ~30s");
}

/// SIGINT, then wait for the graceful exit; returns whether it exited 0.
async fn interrupt_and_wait(mut child: Child) -> bool {
    // SAFETY: `child.id()` is this child's own pid, valid until reaped below.
    let sent = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) } == 0;
    assert!(sent, "failed to send SIGINT to the child process");
    tokio::task::spawn_blocking(move || child.wait())
        .await
        .unwrap()
        .expect("failed to wait on the child process")
        .success()
}

#[tokio::test(flavor = "multi_thread")]
async fn startup_logs_reach_an_otlp_collector_and_the_last_batch_is_flushed_on_shutdown() {
    let _guard = PORT_8000_LOCK.lock().await;
    let (endpoint, received) = collector().await;

    let child = spawn_app(&endpoint, &[], Stdio::null());
    wait_until_ready().await;
    assert!(
        interrupt_and_wait(child).await,
        "graceful shutdown should exit 0"
    );

    let received = received.lock().unwrap();
    let logs: Vec<&Received> = received
        .iter()
        .filter(|request| request.path == "/v1/logs")
        .collect();
    assert!(
        !logs.is_empty(),
        "no OTLP log export reached the collector (got paths: {:?})",
        received.iter().map(|r| r.path.as_str()).collect::<Vec<_>>()
    );
    assert!(logs
        .iter()
        .all(|request| request.content_type.as_deref() == Some("application/x-protobuf")));
    let contains = |needle: &str| {
        logs.iter().any(|request| {
            request
                .body
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
        })
    };
    // Logged right after the bind; exported by the batch thread or, at the
    // latest, by the flush at shutdown.
    assert!(contains("listening on"), "startup record not exported");
    // Logged when the signal arrived, so only the shutdown flush can have
    // carried it if the batch timer hadn't fired yet.
    assert!(
        contains("shutdown signal received"),
        "the last batch was not flushed on shutdown"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_exporter_that_cannot_be_built_falls_back_to_json_logs_only() {
    let _guard = PORT_8000_LOCK.lock().await;
    // Not a recognized OTLP compression algorithm, so the exporter builder
    // fails before any network call (the same trick as the crate's own tests).
    let child = spawn_app(
        "http://127.0.0.1:1",
        &[("OTEL_EXPORTER_OTLP_COMPRESSION", "not-a-real-algorithm")],
        Stdio::piped(),
    );
    let mut child = child;
    let stdout = child.stdout.take().unwrap();
    wait_until_ready().await;
    assert!(
        interrupt_and_wait(child).await,
        "the app must keep running without the export, and exit 0"
    );

    let output = tokio::task::spawn_blocking(move || {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut { stdout }, &mut text).unwrap();
        text
    })
    .await
    .unwrap();
    assert!(
        output.contains("OTLP log exporter setup failed, continuing without it"),
        "stdout: {output}"
    );
    assert!(output.contains("listening on"), "stdout: {output}");
}
