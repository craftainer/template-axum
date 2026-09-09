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

use goose::prelude::*;
use serde_json::{json, Value};

struct Session {
    token: String,
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
    user.set_session_data(Session { token });
    Ok(())
}

fn bearer(user: &GooseUser) -> String {
    user.get_session_data_unchecked::<Session>().token.clone()
}

/// `GET /crud/v1/heroes/v2/json` -- the common-case read path.
async fn list_heroes(user: &mut GooseUser) -> TransactionResult {
    let token = bearer(user);
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

/// Create, update, then delete one record -- the full write path, all
/// three requests against the same owned record so this never
/// accumulates data across a run.
async fn create_update_delete_hero(user: &mut GooseUser) -> TransactionResult {
    let token = bearer(user);

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
    GooseAttack::initialize()?
        .register_scenario(
            scenario!("HeroJourney")
                .register_transaction(transaction!(mint_token).set_on_start())
                .register_transaction(transaction!(list_heroes).set_weight(6)?)
                .register_transaction(transaction!(health_live).set_weight(3)?)
                .register_transaction(transaction!(create_update_delete_hero).set_weight(1)?),
        )
        .execute()
        .await?;

    Ok(())
}
