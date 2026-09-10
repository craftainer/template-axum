//! e2e-equivalent tier (`NFR-0024`, ports reference `NFR-0021`): `reqwest`
//! driving one live `MODE=dev` process and one live `MODE=mock` process
//! over real HTTP -- unlike `tests/postgres_*.rs`/`tests/mqtt_events.rs`,
//! which build the router in-process via `tower::ServiceExt::oneshot`,
//! this spawns the actual compiled binary (`env!("CARGO_BIN_EXE_
//! template-axum")`) and talks to it the way a real client would: over a
//! TCP socket, through `main.rs`'s own `axum::serve` and
//! `into_make_service_with_connect_info`.
//!
//! Both journeys run sequentially in one test function, in one process --
//! `main.rs` binds a fixed `0.0.0.0:8000` (no configurable port), so two
//! server processes bound to it can never run concurrently; running them
//! one after another here, rather than splitting into two files Cargo
//! could schedule as parallel test binaries, is what keeps that true
//! without adding a port setting nothing else in this app needs.
//!
//! Role-journey style, mirroring the reference's per-role `tests/e2e/`
//! split: for each of `viewer`/`editor`/`maintainer`/`detective`/
//! `security`, exercise exactly the operations `FR-0015`/`FR-0033` grant
//! that role, over a real network socket, against a real running server.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

const BASE_URL: &str = "http://127.0.0.1:8000";

/// Owns the spawned `template-axum` child process; killed on drop so a
/// panicking assertion never leaves a server bound to :8000 behind for
/// the next test run.
struct AppProcess(Child);

impl AppProcess {
    fn spawn(mode: &str, extra_env: &[(&str, &str)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_template-axum"));
        command
            .env("MODE", mode)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        for (key, value) in extra_env {
            command.env(key, value);
        }
        let child = command
            .spawn()
            .expect("failed to spawn the template-axum binary");
        Self(child)
    }
}

impl Drop for AppProcess {
    fn drop(&mut self) {
        // A real SIGTERM lets `main.rs`'s `shutdown_signal` drain and the
        // process exit normally -- which is what flushes its LLVM
        // coverage profile, unlike `.kill()` (SIGKILL, no cleanup ever
        // runs). Falls back to `.kill()` if the process doesn't exit
        // within ~2s, so a stuck shutdown can never hang the suite.
        // SAFETY: `self.0.id()` is this child's own pid, valid until
        // reaped below.
        let killed_cleanly = unsafe { libc::kill(self.0.id() as libc::pid_t, libc::SIGTERM) } == 0;
        if killed_cleanly {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            loop {
                match self.0.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    _ => break,
                }
            }
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Polls `path` until it returns a success status, or panics after ~30s --
/// generous, since every backing service `MODE=dev` needs is already a
/// running sibling container by the time this test starts.
async fn wait_until_ready(client: &reqwest::Client, path: &str) {
    for _ in 0..60 {
        if let Ok(response) = client.get(format!("{BASE_URL}{path}")).send().await {
            if response.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("template-axum did not become ready at {path} within ~30s");
}

async fn mock_token(client: &reqwest::Client, sub: &str, roles: &[&str]) -> String {
    let response = client
        .post(format!("{BASE_URL}/mock/token"))
        .json(&json!({ "sub": sub, "roles": roles }))
        .send()
        .await
        .expect("POST /mock/token failed");
    assert_eq!(response.status(), 200, "POST /mock/token should succeed");
    let body: Value = response.json().await.unwrap();
    body["access_token"].as_str().unwrap().to_string()
}

/// Real Keycloak Resource Owner Password Credentials grant -- the `api`
/// client is public with `directAccessGrantsEnabled` (`realm-export.json`),
/// and every test user's password equals its username
/// (`.devcontainer/stack/keycloak/README.md`).
async fn keycloak_token(client: &reqwest::Client, username: &str) -> String {
    let token_url = std::env::var("OIDC_TOKEN_URL").unwrap_or_else(|_| {
        "http://keycloak:8080/realms/template-axum/protocol/openid-connect/token".to_string()
    });
    let client_id = std::env::var("OIDC_CLIENT_ID").unwrap_or_else(|_| "api".to_string());
    let response = client
        .post(token_url)
        .form(&[
            ("grant_type", "password"),
            ("client_id", client_id.as_str()),
            ("username", username),
            ("password", username),
        ])
        .send()
        .await
        .expect("Keycloak token request failed -- is the devcontainer stack running?");
    assert_eq!(
        response.status(),
        200,
        "Keycloak should issue a token for test user {username}"
    );
    let body: Value = response.json().await.unwrap();
    body["access_token"].as_str().unwrap().to_string()
}

enum Backend {
    Dev,
    Mock,
}

async fn token_for(client: &reqwest::Client, backend: &Backend, role: &str) -> String {
    match backend {
        Backend::Dev => keycloak_token(client, role).await,
        Backend::Mock => mock_token(client, role, &[role]).await,
    }
}

/// The role journey shared by both `MODE`s: `viewer` can only read,
/// `editor` can create/update its own record, `maintainer` alone can
/// delete, `security` (not `viewer`) can call `/audit`.
async fn hero_and_audit_role_journey(client: &reqwest::Client, backend: Backend) {
    let viewer = token_for(client, &backend, "viewer").await;
    let editor = token_for(client, &backend, "editor").await;
    let maintainer = token_for(client, &backend, "maintainer").await;
    let detective = token_for(client, &backend, "detective").await;
    let security = token_for(client, &backend, "security").await;

    // viewer: read-only.
    let list = client
        .get(format!("{BASE_URL}/crud/v1/heroes/v2/json"))
        .bearer_auth(&viewer)
        .send()
        .await
        .unwrap();
    assert_eq!(list.status(), 200, "viewer should be able to list heroes");

    let forbidden_create = client
        .post(format!("{BASE_URL}/crud/v1/heroes/v2/json"))
        .bearer_auth(&viewer)
        .json(&json!({"name": "Spectra", "powers": ["flight"], "power_level": 5}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        forbidden_create.status(),
        403,
        "viewer must not be able to create"
    );

    // editor: has the write role -- proven against a nonexistent id (role
    // check runs before the not-found lookup) rather than by creating a
    // record this journey would have no way to clean up: each real
    // Keycloak test user is a distinct `sub` (ownership-scoped, ADR 0007),
    // and editor holds no delete role at all to remove what it creates.
    let editor_update = client
        .patch(format!("{BASE_URL}/crud/v1/heroes/v2/json?id=999999999"))
        .bearer_auth(&editor)
        .json(&json!({"power_level": 9}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        editor_update.status(),
        404,
        "editor has the write role -- role check passes, id just doesn't exist"
    );

    // detective: read-only -- proven the same way, no delete role at all.
    let detective_delete = client
        .delete(format!("{BASE_URL}/crud/v1/heroes/v2/json?id=999999999"))
        .bearer_auth(&detective)
        .send()
        .await
        .unwrap();
    assert_eq!(
        detective_delete.status(),
        403,
        "detective must not be able to delete"
    );

    // maintainer: in HERO_WRITE_ROLES *and* HERO_DELETE_ROLES, so it's the
    // one role journey that can create, update, and delete its own record
    // end to end without leaving anything behind in a real, shared
    // database.
    let created = client
        .post(format!("{BASE_URL}/crud/v1/heroes/v2/json"))
        .bearer_auth(&maintainer)
        .json(&json!({"name": "Spectra", "powers": ["flight"], "power_level": 5}))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201, "maintainer should be able to create");
    let created: Value = created.json().await.unwrap();
    let id = created["id"].as_i64().unwrap();

    let updated = client
        .patch(format!("{BASE_URL}/crud/v1/heroes/v2/json?id={id}"))
        .bearer_auth(&maintainer)
        .json(&json!({"power_level": 9}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        updated.status(),
        200,
        "maintainer should be able to update its own record"
    );

    let deleted = client
        .delete(format!("{BASE_URL}/crud/v1/heroes/v2/json?id={id}"))
        .bearer_auth(&maintainer)
        .send()
        .await
        .unwrap();
    assert_eq!(
        deleted.status(),
        204,
        "maintainer should be able to delete its own record"
    );

    // /audit: security in, viewer out (FR-0033).
    let audit = client
        .get(format!("{BASE_URL}/audit"))
        .bearer_auth(&security)
        .send()
        .await
        .unwrap();
    assert_eq!(
        audit.status(),
        200,
        "security should be able to call /audit"
    );
    let audit_body: Value = audit.json().await.unwrap();
    // Keycloak's `sub` is the user's internal UUID, not their username
    // (`Mode::Mock`'s minted tokens use the role name as `sub` directly)
    // -- assert on `roles` instead, which is comparable across both
    // backends.
    assert!(
        audit_body["subject"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "audit response should report a non-empty subject"
    );
    let roles: Vec<&str> = audit_body["roles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        roles.contains(&"security"),
        "audit response should report the caller's own granted roles, got {roles:?}"
    );

    let audit_forbidden = client
        .get(format!("{BASE_URL}/audit"))
        .bearer_auth(&viewer)
        .send()
        .await
        .unwrap();
    assert_eq!(
        audit_forbidden.status(),
        403,
        "viewer must not be able to call /audit"
    );
}

#[tokio::test]
async fn dev_and_mock_role_journeys_run_against_a_real_live_server() {
    let client = reqwest::Client::new();

    // -- MODE=dev: real Postgres/Redis/S3/MQTT/Keycloak, real tokens. --
    {
        let _app = AppProcess::spawn("dev", &[]);
        wait_until_ready(&client, "/health/ready").await;
        hero_and_audit_role_journey(&client, Backend::Dev).await;
    }

    // -- MODE=mock: zero containers, tokens minted by the app itself. --
    {
        let _app = AppProcess::spawn("mock", &[("ALLOW_MOCK_MODE", "1")]);
        wait_until_ready(&client, "/health/live").await;
        hero_and_audit_role_journey(&client, Backend::Mock).await;
    }
}
