//! `main.rs`'s two failure branches: an invalid `MODE` (`Settings::
//! from_env`'s error branch, then `std::process::exit(1)`) and a port
//! already in use (`TcpListener::bind`'s `.expect()`). Both let the
//! spawned child exit **on its own** -- a panic/`exit()` both run libc's
//! `atexit` handlers, which is what flushes the LLVM coverage profile,
//! unlike `tests/e2e.rs`'s `AppProcess`, which is killed on drop and
//! never reaches either.

use std::process::{Command, Stdio};
use std::time::Duration;

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
