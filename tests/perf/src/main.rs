//! Perf tier (`NFR-0024` tiers, `NFR-0030` reporting, `docs/adrs/0018`): a `goose` load test against a
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
//!
//! After the run, a JSON summary (schema: this directory's `README.md`) is
//! written to `PERF_SUMMARY_FILE` (default `perf-summary.json`; set but
//! empty skips it). When `PERF_CGROUP_CPU_STAT` names the app container's
//! cgroup v2 `cpu.stat`, that file is sampled every `PERF_CPU_SAMPLE_MS`
//! (default 500) and the CPU used over the measured window is included.

mod cpu;
mod summary;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use goose::metrics::GooseMetrics;
use goose::prelude::*;
use goose::test_plan::TestPlanStepAction;
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

/// Reduce goose's metrics to the plain types `summary` works on.
fn endpoint_stats(metrics: &GooseMetrics) -> Vec<summary::EndpointStats> {
    metrics
        .requests
        .values()
        .map(|request| {
            let data = &request.raw_data;
            summary::EndpointStats {
                method: request.method.to_string(),
                path: request.path.clone(),
                success: request.success_count as u64,
                failed: request.fail_count as u64,
                times: data
                    .times
                    .iter()
                    .map(|(ms, count)| (*ms as u64, *count as u64))
                    .collect::<BTreeMap<_, _>>(),
                max_ms: data.maximum_time as u64,
                total_ms: data.total_time as u64,
            }
        })
        .collect()
}

fn window_of(metrics: &GooseMetrics) -> Option<summary::Window> {
    let history: Vec<(summary::Phase, u64)> = metrics
        .history
        .iter()
        .map(|step| {
            let phase = match step.action {
                TestPlanStepAction::Increasing => summary::Phase::Increasing,
                TestPlanStepAction::Maintaining => summary::Phase::Maintaining,
                _ => summary::Phase::Other,
            };
            (phase, step.timestamp.timestamp_millis().max(0) as u64)
        })
        .collect();
    summary::Window::from_history(&history)
}

/// Write the JSON summary for a finished run (see the module docs).
fn write_summary(
    metrics: &GooseMetrics,
    scenario: &str,
    mode: &str,
    cpu_samples: &[cpu::Sample],
    sample_interval: Duration,
) {
    let path = env_or("PERF_SUMMARY_FILE", "perf-summary.json");
    if path.is_empty() {
        return;
    }
    let Some(window) = window_of(metrics) else {
        eprintln!("no test-plan history; not writing {path}");
        return;
    };
    let cpu_usage = if cpu_samples.is_empty() {
        None
    } else {
        let usage = cpu::window_usage(cpu_samples, window.start_ms, window.end_ms);
        if usage.is_none() {
            eprintln!("cpu.stat samples do not cover the measured window; cpu is null");
        }
        usage
    };
    let run = summary::RunInfo {
        scenario: scenario.to_string(),
        mode: mode.to_string(),
        users: metrics.maximum_users as u64,
        git_sha: env_or("PERF_GIT_SHA", "unknown"),
        image_tag: env_or("PERF_IMAGE", "unknown"),
        cpu_limit_cores: std::env::var("PERF_CPUS")
            .ok()
            .and_then(|v| v.trim().parse().ok()),
        memory_limit_mib: std::env::var("PERF_MEMORY")
            .ok()
            .and_then(|v| summary::parse_memory_mib(&v)),
        cpu_sample_interval_ms: sample_interval.as_millis() as u64,
    };
    let document =
        summary::build_summary(&run, &window, &endpoint_stats(metrics), cpu_usage.as_ref());
    let text = serde_json::to_string_pretty(&document).expect("the summary should serialize");
    match std::fs::write(&path, text + "\n") {
        Ok(()) => eprintln!("wrote {path}"),
        Err(err) => eprintln!("could not write {path}: {err}"),
    }
}

#[tokio::main]
async fn main() -> Result<(), GooseError> {
    let target = env_or("PERF_TARGET", "mock");
    let scenario_name = match target.as_str() {
        "real" => "HeroJourneyReal",
        _ => "HeroJourney",
    };
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

    let sample_interval = Duration::from_millis(
        std::env::var("PERF_CPU_SAMPLE_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|ms| *ms > 0)
            .unwrap_or(500),
    );
    let sampler = std::env::var("PERF_CGROUP_CPU_STAT")
        .ok()
        .filter(|path| !path.is_empty())
        .map(|path| cpu::Sampler::start(PathBuf::from(path), sample_interval));

    let metrics = attack.execute().await?;

    let cpu_samples = match sampler {
        Some(sampler) => sampler.stop().await,
        None => Vec::new(),
    };
    write_summary(
        &metrics,
        scenario_name,
        &target,
        &cpu_samples,
        sample_interval,
    );

    Ok(())
}
