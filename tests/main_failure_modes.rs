//! `main.rs`'s two failure branches: an invalid `MODE` (`Settings::
//! from_env`'s error branch, then `std::process::exit(1)`) and a port
//! already in use (`TcpListener::bind`'s `.expect()`). Both let the
//! spawned child exit **on its own** -- a panic/`exit()` both run libc's
//! `atexit` handlers, which is what flushes the LLVM coverage profile,
//! unlike `tests/e2e.rs`'s `AppProcess`, which is killed on drop and
//! never reaches either.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use tokio::sync::Mutex;

/// `a_port_already_in_use_panics` and `sigint_triggers_a_graceful_shutdown`
/// both bind/spawn against the fixed `0.0.0.0:8000` `main.rs` always
/// uses -- Rust's test harness runs `#[tokio::test]`s in the same binary
/// concurrently by default, so this serializes the two. An async-aware
/// `Mutex` (unlike `config.rs`'s own `std::sync::Mutex` `ENV_LOCK`,
/// which never holds its guard across an `.await`) since both tests hold
/// the guard across several.
static PORT_8000_LOCK: Mutex<()> = Mutex::const_new(());

#[tokio::test]
async fn an_invalid_mode_exits_with_status_1() {
    let output = tokio::task::spawn_blocking(|| {
        Command::new(env!("CARGO_BIN_EXE_template-axum"))
            .env("MODE", "bogus")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .expect("failed to spawn the template-axum binary")
    })
    .await
    .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid configuration"), "stderr: {stderr}");
}

#[tokio::test]
async fn a_port_already_in_use_panics() {
    let _guard = PORT_8000_LOCK.lock().await;
    // Bind 0.0.0.0:8000 ourselves first, so the child's own
    // `TcpListener::bind` fails its `.expect()` and panics -- letting the
    // child process exit on its own (rather than being killed) is what
    // flushes its coverage profile.
    let _blocker = tokio::net::TcpListener::bind("0.0.0.0:8000")
        .await
        .expect("failed to bind 0.0.0.0:8000 for the test itself");

    let child = Command::new(env!("CARGO_BIN_EXE_template-axum"))
        .env("MODE", "mock")
        .env("ALLOW_MOCK_MODE", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn the template-axum binary");

    let output = tokio::task::spawn_blocking(move || child.wait_with_output())
        .await
        .unwrap()
        .expect("failed to wait on the child process");

    drop(_blocker);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to bind 0.0.0.0:8000"),
        "stderr: {stderr}"
    );
}

/// `main.rs`'s `shutdown_signal` races Ctrl-C against `SIGTERM`
/// (`tests/e2e.rs`'s `AppProcess::drop` already exercises the `SIGTERM`
/// arm); this covers the Ctrl-C (`SIGINT`) arm the same way.
#[tokio::test]
async fn sigint_triggers_a_graceful_shutdown() {
    let _guard = PORT_8000_LOCK.lock().await;
    let mut child = Command::new(env!("CARGO_BIN_EXE_template-axum"))
        .env("MODE", "mock")
        .env("ALLOW_MOCK_MODE", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("failed to spawn the template-axum binary");

    let client = reqwest::Client::new();
    let mut ready = false;
    for _ in 0..60 {
        if let Ok(response) = client.get("http://127.0.0.1:8000/health/live").send().await {
            if response.status().is_success() {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(ready, "template-axum did not become ready within ~30s");

    // SAFETY: `child.id()` is this child's own pid, valid until reaped
    // below.
    let sent = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) } == 0;
    assert!(sent, "failed to send SIGINT to the child process");

    let status = tokio::task::spawn_blocking(move || child.wait())
        .await
        .unwrap()
        .expect("failed to wait on the child process");
    assert!(
        status.success(),
        "graceful shutdown should exit 0: {status:?}"
    );
}

// -- graceful shutdown (docs/adrs/0027) ------------------------------------

const BASE_URL: &str = "http://127.0.0.1:8000";
const HERO_BODY: &str = r#"{"name":"Spectra","powers":["flight"],"power_level":5}"#;

/// Spawns the binary in mock mode on :8000 and waits until it is ready.
/// Callers hold `PORT_8000_LOCK`.
async fn start_mock_server(shutdown_timeout_seconds: Option<&str>) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_template-axum"));
    command
        .env("MODE", "mock")
        .env("ALLOW_MOCK_MODE", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if let Some(seconds) = shutdown_timeout_seconds {
        command.env("GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS", seconds);
    }
    let mut child = command
        .spawn()
        .expect("failed to spawn the template-axum binary");

    let client = reqwest::Client::new();
    let mut ready = false;
    for _ in 0..60 {
        if let Ok(response) = client.get(format!("{BASE_URL}/health/live")).send().await {
            if response.status().is_success() {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    if !ready {
        child.kill().ok();
        child.wait().ok();
        panic!("template-axum did not become ready within ~30s");
    }
    child
}

fn send_signal(child: &Child, signal: libc::c_int) {
    // SAFETY: `child.id()` is this child's own pid, valid until reaped.
    let sent = unsafe { libc::kill(child.id() as libc::pid_t, signal) } == 0;
    assert!(sent, "failed to send signal {signal} to the child process");
}

/// Waits for the child to exit, killing it (and failing) past `limit`.
async fn wait_for_exit(
    mut child: Child,
    limit: Duration,
) -> (std::process::ExitStatus, Duration, String) {
    tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait().expect("failed to poll the child") {
                let mut stdout = String::new();
                if let Some(mut out) = child.stdout.take() {
                    std::io::Read::read_to_string(&mut out, &mut stdout).unwrap();
                }
                return (status, started.elapsed(), stdout);
            }
            if started.elapsed() > limit {
                child.kill().ok();
                panic!("the process was still running {limit:?} after the signal");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    })
    .await
    .unwrap()
}

async fn mock_token(client: &reqwest::Client, roles: &[&str]) -> String {
    let body: serde_json::Value = client
        .post(format!("{BASE_URL}/mock/token"))
        .json(&serde_json::json!({ "sub": "shutdown-test", "roles": roles }))
        .send()
        .await
        .expect("POST /mock/token failed")
        .json()
        .await
        .unwrap();
    body["access_token"].as_str().unwrap().to_string()
}

/// Opens a hero create whose body is only half sent, so the request is
/// in flight (accepted, handler waiting on the body) until the test sends
/// the rest.
async fn begin_partial_create(token: &str) -> TcpStream {
    let split = HERO_BODY.len() / 2;
    let mut stream = TcpStream::connect("127.0.0.1:8000").await.unwrap();
    let head = format!(
        "POST /crud/v1/heroes/v2/json HTTP/1.1\r\nHost: localhost\r\n\
         Authorization: Bearer {token}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        HERO_BODY.len()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    stream
        .write_all(&HERO_BODY.as_bytes()[..split])
        .await
        .unwrap();
    stream.flush().await.unwrap();
    stream
}

/// SIGTERM mid-run: an open `/events` stream ends, a request accepted before
/// the signal still completes (no 5xx), and the process exits 0 inside the
/// timeout.
#[tokio::test]
async fn sigterm_ends_event_streams_and_lets_accepted_requests_finish() {
    let _guard = PORT_8000_LOCK.lock().await;
    let child = start_mock_server(Some("20")).await;
    let client = reqwest::Client::new();
    let token = mock_token(&client, &["viewer", "editor"]).await;

    let mut events = client
        .get(format!("{BASE_URL}/crud/v1/heroes/v2/json/events"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(events.status(), 200);
    // The first frame announces the subscriber id: the stream is live.
    assert!(events.chunk().await.unwrap().is_some());

    let mut in_flight = begin_partial_create(&token).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    send_signal(&child, libc::SIGTERM);

    // The stream ends (rather than being held open until the timeout).
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        while events.chunk().await.unwrap().is_some() {}
    })
    .await;
    assert!(
        ended.is_ok(),
        "the /events stream was still open 5s after SIGTERM"
    );

    // The accepted request is still served once its body arrives.
    in_flight
        .write_all(&HERO_BODY.as_bytes()[HERO_BODY.len() / 2..])
        .await
        .unwrap();
    let mut response = String::new();
    in_flight.read_to_string(&mut response).await.unwrap();
    assert!(
        response.starts_with("HTTP/1.1 201"),
        "an accepted request must complete, got: {response}"
    );

    let (status, elapsed, _stdout) = wait_for_exit(child, Duration::from_secs(20)).await;
    assert!(
        status.success(),
        "graceful shutdown should exit 0: {status:?}"
    );
    assert!(elapsed < Duration::from_secs(10), "took {elapsed:?}");
}

/// A request that outlives `GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS` does not hold
/// the process open: it logs the timeout and exits 0 (pool close and the
/// OTel flush still run after it).
#[tokio::test]
async fn a_request_outliving_the_timeout_does_not_block_exit() {
    let _guard = PORT_8000_LOCK.lock().await;
    let child = start_mock_server(Some("1")).await;
    let client = reqwest::Client::new();
    let token = mock_token(&client, &["editor"]).await;

    // Never completed: the handler waits on the rest of the body forever.
    let _stalled = begin_partial_create(&token).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    send_signal(&child, libc::SIGTERM);

    let (status, elapsed, stdout) = wait_for_exit(child, Duration::from_secs(15)).await;
    assert!(
        status.success(),
        "the timeout path should exit 0: {status:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(900),
        "exited in {elapsed:?}, before the 1s timeout: the stalled request was not waited for"
    );
    assert!(
        stdout.contains("graceful shutdown timed out"),
        "stdout: {stdout}"
    );
}

#[tokio::test]
async fn an_invalid_shutdown_timeout_exits_with_status_1() {
    let output = tokio::task::spawn_blocking(|| {
        Command::new(env!("CARGO_BIN_EXE_template-axum"))
            .env("MODE", "mock")
            .env("ALLOW_MOCK_MODE", "1")
            .env("GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS", "0")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .expect("failed to spawn the template-axum binary")
    })
    .await
    .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS"),
        "stderr: {stderr}"
    );
}
