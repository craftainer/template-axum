//! Perf tier (`NFR-0024`, `docs/adrs/0018`): a `goose` load test against a
//! *running* `template-axum` server -- the `runner`-stage Docker image
//! specifically (never a `cargo run` dev loop), per this directory's own
//! `README.md`. Points at `--host` (goose's own CLI convention, e.g.
//! `--host http://localhost:8000`); nothing here builds or starts the
//! server itself.
//!
//! Each simulated user mints its own `MODE=mock` bearer token
//! (`maintainer` role -- the one role in `HERO_WRITE_ROLES` *and*
//! `HERO_DELETE_ROLES`, so one user can create, update, and delete its
//! own records without hitting `docs/adrs/0007`'s ownership scoping) once
//! in `on_start`, then runs a weighted mix of read and write requests.
//!
//! Two targets, picked by the `PERF_TARGET` environment variable (goose
//! owns the command line, so extra options go through the environment):
//!
//! - `mock` (default): the `HeroJourney` scenario against a `MODE=mock`
//!   server. No backing services; tokens come from `POST /mock/token`.
//! - `real`: the `HeroJourneyReal` scenario against a server wired to real
//!   Postgres, Redis and MQTT (`MODE=dev`). Tokens are real Keycloak
//!   access tokens from the OIDC password grant (`PERF_TOKEN_URL`,
//!   `PERF_USERNAME`, `PERF_PASSWORD`, `PERF_CLIENT_ID`); the server
//!   validates them exactly as in production, so this needs no auth
//!   bypass. It adds the stats read, which the mock scenario skips.

use std::time::{Duration, Instant};

use goose::prelude::*;
use serde_json::{json, Value};

struct Session {
    token: String,
    /// Real mode only: when to fetch a fresh token (mock tokens never
    /// expire as far as the server is concerned).
    refresh_at: Option<Instant>,
}

/// Env var with a default, for the real-mode settings below.
fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Real mode: fetch a Keycloak access token with the OIDC password grant
/// (`directAccessGrantsEnabled` on the stack's public `api` client). Goes
/// through `user.client` directly because the token endpoint is not on the
/// goose `--host`.
async fn fetch_real_token(user: &mut GooseUser) -> TransactionResult {
    let token_url = env_or(
        "PERF_TOKEN_URL",
        "http://keycloak:8080/realms/template-axum/protocol/openid-connect/token",
    );
    let form = [
        ("grant_type", "password".to_string()),
        ("client_id", env_or("PERF_CLIENT_ID", "api")),
        ("username", env_or("PERF_USERNAME", "maintainer")),
        ("password", env_or("PERF_PASSWORD", "maintainer")),
    ];
    let response = user
        .client
        .post(&token_url)
        .form(&form)
        .send()
        .await
        .map_err(|err| Box::new(err.into()))?
        .error_for_status()
        .map_err(|err| Box::new(err.into()))?;
    let body: Value = response.json().await.map_err(|err| Box::new(err.into()))?;
    // goose's TransactionError has no variant for a malformed body; the
    // loud panic names the misconfiguration (wrong client or realm).
    let token = body["access_token"]
        .as_str()
        .expect("the token endpoint should return access_token")
        .to_string();
    let lifetime = body["expires_in"].as_u64().unwrap_or(60);
    // Refresh a little early so an in-flight request never carries an
    // expired token: 30s early, but never earlier than half the lifetime,
    // so a short-lived token cannot make every request re-fetch one.
    let margin = 30.min(lifetime / 2);
    let refresh_at = Instant::now() + Duration::from_secs(lifetime - margin);
    user.set_session_data(Session {
        token,
        refresh_at: Some(refresh_at),
    });
    Ok(())
}

async fn mint_token(user: &mut GooseUser) -> TransactionResult {
    let goose = user
        .post_json(
            "/mock/token",
            &json!({"sub": "perf", "roles": ["maintainer"]}),
        )
        .await?;
    let response = goose.response.map_err(|err| Box::new(err.into()))?;
    let body: Value = response
        .json()
        .await
        .expect("POST /mock/token should return a JSON body");
    let token = body["access_token"]
        .as_str()
        .expect("POST /mock/token should return access_token")
        .to_string();
    user.set_session_data(Session {
        token,
        refresh_at: None,
    });
    Ok(())
}

/// The user's current bearer token, re-fetching a real one first if it is
/// about to expire (a long run outlives Keycloak's access-token lifespan).
async fn bearer(user: &mut GooseUser) -> Result<String, Box<TransactionError>> {
    let stale = user
        .get_session_data_unchecked::<Session>()
        .refresh_at
        .is_some_and(|at| Instant::now() >= at);
    if stale {
        fetch_real_token(user).await?;
    }
    Ok(user.get_session_data_unchecked::<Session>().token.clone())
}

/// `GET /crud/v1/heroes/v2/json` -- the common-case read path.
async fn list_heroes(user: &mut GooseUser) -> TransactionResult {
    let token = bearer(user).await?;
    let request_builder = user
        .get_request_builder(&GooseMethod::Get, "/crud/v1/heroes/v2/json")?
        .bearer_auth(token);
    let goose_request = GooseRequest::builder()
        .method(GooseMethod::Get)
        .path("/crud/v1/heroes/v2/json")
        .set_request_builder(request_builder)
        .build();
    user.request(goose_request).await?;
    Ok(())
}

/// `GET /health/live` -- cheap, dependency-free, a useful floor/ceiling
/// comparison against the Hero routes' own latency.
async fn health_live(user: &mut GooseUser) -> TransactionResult {
    user.get("/health/live").await?;
    Ok(())
}

/// `GET /crud/v1/heroes/v2/json/stats` -- an aggregate over the whole
/// table, only meaningful against a real database (real mode only).
async fn stats(user: &mut GooseUser) -> TransactionResult {
    let token = bearer(user).await?;
    let request_builder = user
        .get_request_builder(&GooseMethod::Get, "/crud/v1/heroes/v2/json/stats")?
        .bearer_auth(token);
    let goose_request = GooseRequest::builder()
        .method(GooseMethod::Get)
        .path("/crud/v1/heroes/v2/json/stats")
        .set_request_builder(request_builder)
        .build();
    user.request(goose_request).await?;
    Ok(())
}

/// Create one record and leave it in place. Only the real-mode seed
/// scenario runs this, so the journey's reads and `stats` aggregate over a
/// non-trivial table.
async fn seed_hero(user: &mut GooseUser) -> TransactionResult {
    let token = bearer(user).await?;
    let builder = user
        .get_request_builder(&GooseMethod::Post, "/crud/v1/heroes/v2/json")?
        .bearer_auth(token)
        .json(&json!({"name": "Seed", "powers": ["speed", "flight"], "power_level": 3}));
    let request = GooseRequest::builder()
        .method(GooseMethod::Post)
        .path("/crud/v1/heroes/v2/json")
        .set_request_builder(builder)
        .build();
    user.request(request).await?;
    Ok(())
}

/// Create, update, then delete one record -- the full write path, all
/// three requests against the same owned record so this never
/// accumulates data across a run.
async fn create_update_delete_hero(user: &mut GooseUser) -> TransactionResult {
    let token = bearer(user).await?;

    let create_builder = user
        .get_request_builder(&GooseMethod::Post, "/crud/v1/heroes/v2/json")?
        .bearer_auth(&token)
        .json(&json!({"name": "Loadtest", "powers": ["speed"], "power_level": 1}));
    let create_request = GooseRequest::builder()
        .method(GooseMethod::Post)
        .path("/crud/v1/heroes/v2/json")
        .set_request_builder(create_builder)
        .build();
    let created = user.request(create_request).await?;
    let response = created.response.map_err(|err| Box::new(err.into()))?;
    let body: Value = response
        .json()
        .await
        .expect("POST /crud/v1/heroes/v2/json should return a JSON body");
    let id = body["id"].as_i64().expect("created hero should have an id");

    let update_path = format!("/crud/v1/heroes/v2/json?id={id}");
    let update_builder = user
        .get_request_builder(&GooseMethod::Patch, &update_path)?
        .bearer_auth(&token)
        .json(&json!({"power_level": 2}));
    let update_request = GooseRequest::builder()
        .method(GooseMethod::Patch)
        .path("/crud/v1/heroes/v2/json")
        .set_request_builder(update_builder)
        .build();
    user.request(update_request).await?;

    let delete_builder = user
        .get_request_builder(&GooseMethod::Delete, &update_path)?
        .bearer_auth(&token);
    let delete_request = GooseRequest::builder()
        .method(GooseMethod::Delete)
        .path("/crud/v1/heroes/v2/json")
        .set_request_builder(delete_builder)
        .build();
    user.request(delete_request).await?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), GooseError> {
    let target = env_or("PERF_TARGET", "mock");
    let scenario = match target.as_str() {
        "mock" => scenario!("HeroJourney")
            .register_transaction(transaction!(mint_token).set_on_start())
            .register_transaction(transaction!(list_heroes).set_weight(6)?)
            .register_transaction(transaction!(health_live).set_weight(3)?)
            .register_transaction(transaction!(create_update_delete_hero).set_weight(1)?),
        "real" => scenario!("HeroJourneyReal")
            .register_transaction(transaction!(fetch_real_token).set_on_start())
            .register_transaction(transaction!(list_heroes).set_weight(6)?)
            .register_transaction(transaction!(stats).set_weight(2)?)
            .register_transaction(transaction!(health_live).set_weight(3)?)
            .register_transaction(transaction!(create_update_delete_hero).set_weight(1)?),
        other => {
            eprintln!("PERF_TARGET must be \"mock\" or \"real\", got {other:?}");
            std::process::exit(2);
        }
    };

    let mut attack = GooseAttack::initialize()?.register_scenario(scenario);
    if target == "real" {
        // Run on its own: `--scenarios HeroSeedReal --users N --iterations M`
        // creates N*M records through the API (see this directory's README).
        attack = attack.register_scenario(
            scenario!("HeroSeedReal")
                .register_transaction(transaction!(fetch_real_token).set_on_start())
                .register_transaction(transaction!(seed_hero)),
        );
    }
    attack.execute().await?;

    Ok(())
}
