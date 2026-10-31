//! Product analytics (`docs/adrs/0029`, FR-0037, NFR-0033) over the
//! mock-mode router and a stand-in for Umami's `POST /api/batch`: needs no
//! backing service, and never the real Umami.
//!
//! One test function, because it changes the process environment (the
//! library reads `ANALYTICS_*` itself) and the process-wide analytics
//! client, which the other tests in a binary would race with. Each scenario
//! sets the environment, drives the router, flushes with
//! `analytics::shutdown()` (which also discards the client, so the next
//! scenario reads the environment again) and inspects what the stand-in
//! received.

mod common;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use template_axum::config::Settings;
use template_axum::{build_router, build_state};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tower::ServiceExt;

type Received = Arc<Mutex<Vec<Value>>>;

/// A local HTTP server that records every item of every posted batch and
/// answers like Umami (`{"errors":0}`).
async fn fake_umami() -> (String, Received) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let received: Received = Arc::default();
    let sink = received.clone();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let sink = sink.clone();
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 4096];
                let body = loop {
                    let read = socket.read(&mut chunk).await.unwrap();
                    if read == 0 {
                        return;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                    let text = String::from_utf8_lossy(&buffer).into_owned();
                    let Some((head, rest)) = text.split_once("\r\n\r\n") else {
                        continue;
                    };
                    let length = head
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    if rest.len() >= length {
                        break rest[..length].to_string();
                    }
                };
                if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&body) {
                    sink.lock().unwrap().extend(items);
                }
                let reply = "{\"errors\":0}";
                let _ = socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                            reply.len()
                        )
                        .as_bytes(),
                    )
                    .await;
            });
        }
    });
    (url, received)
}

fn with_peer(mut request: Request<Body>) -> Request<Body> {
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(SocketAddr::from((
            [127, 0, 0, 1],
            40000,
        ))));
    request
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn token(app: &Router, roles: &[&str]) -> String {
    let response = app
        .clone()
        .oneshot(with_peer(
            Request::builder()
                .method("POST")
                .uri("/mock/token")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({"sub": "alice", "roles": roles}).to_string(),
                ))
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    json_body(response).await["access_token"]
        .as_str()
        .unwrap()
        .to_string()
}

/// `POST` a hero (or `DELETE ?id=`) with `headers`, returning the status.
async fn call(
    app: &Router,
    token: &str,
    method: &str,
    uri: &str,
    body: Value,
    headers: &[(&str, &str)],
) -> StatusCode {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let body = if body.is_null() {
        Body::empty()
    } else {
        Body::from(body.to_string())
    };
    app.clone()
        .oneshot(with_peer(builder.body(body).unwrap()))
        .await
        .unwrap()
        .status()
}

const CREATE: &str = "/crud/v1/heroes/v2/json";

/// Flush and discard the process-wide analytics client.
async fn flush() {
    tokio::task::spawn_blocking(craftainer_observability::analytics::shutdown)
        .await
        .unwrap();
}

fn set_env(url: &str, extra: &[(&str, &str)]) {
    // SAFETY: the only test in this binary, so nothing reads the
    // environment concurrently.
    unsafe {
        std::env::set_var("ANALYTICS_BACKEND", "umami");
        std::env::set_var("ANALYTICS_ENDPOINT", url);
        std::env::set_var("ANALYTICS_SITE_ID", "a076165c-04f6-5693-94ad-a3ae8ca0c316");
        std::env::set_var("ANALYTICS_HOSTNAME", "template-axum.craftainer.local");
        std::env::set_var("ANALYTICS_PROP_KEYS", "powers_count");
        std::env::remove_var("ANALYTICS_CONSENT");
        std::env::remove_var("ANALYTICS_RESPECT_GPC");
        for (key, value) in extra {
            std::env::set_var(key, value);
        }
    }
}

fn payloads(received: &Received) -> Vec<Value> {
    received
        .lock()
        .unwrap()
        .drain(..)
        .map(|item| {
            assert_eq!(item["type"], "event");
            item["payload"].clone()
        })
        .collect()
}

#[tokio::test]
async fn hero_events_honour_gpc_dnt_consent_and_the_prop_allowlist() {
    unsafe {
        std::env::set_var("MODE", "mock");
        std::env::set_var("ALLOW_MOCK_MODE", "1");
    }
    let (url, received) = fake_umami().await;
    let settings = Settings::from_env().expect("mock settings");
    let app = build_router(build_state(Arc::new(settings)).await);
    let editor = token(&app, &["editor"]).await;
    let maintainer = token(&app, &["maintainer"]).await;
    let hero = json!({"name": "Spectra", "powers": ["flight", "x-ray vision"]});

    // Default (implied consent): a create is tracked once, after it succeeded.
    set_env(&url, &[]);
    assert_eq!(
        call(&app, &editor, "POST", CREATE, hero.clone(), &[]).await,
        StatusCode::CREATED
    );
    flush().await;
    let events = payloads(&received);
    assert_eq!(events.len(), 1, "{events:?}");
    let event = &events[0];
    assert_eq!(event["name"], "hero.created");
    assert_eq!(event["url"], CREATE, "the constant route path");
    assert_eq!(event["ip"], "server", "never a client address");
    assert_eq!(event["hostname"], "template-axum.craftainer.local");
    assert_eq!(event["website"], "a076165c-04f6-5693-94ad-a3ae8ca0c316");
    assert_eq!(
        event["data"],
        json!({"powers_count": 2}),
        "a count only: no names, ids or owners"
    );

    // A delete is tracked too, with no properties and the route path, not
    // the `?id=` it was called with.
    set_env(&url, &[]);
    assert_eq!(
        call(
            &app,
            &maintainer,
            "DELETE",
            &format!("{CREATE}?id=1"),
            Value::Null,
            &[]
        )
        .await,
        StatusCode::NO_CONTENT
    );
    flush().await;
    let events = payloads(&received);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["name"], "hero.deleted");
    assert_eq!(events[0]["url"], CREATE);
    assert!(events[0].get("data").is_none());

    // GPC and DNT each drop the event; the write itself still succeeds.
    for header in [("Sec-GPC", "1"), ("DNT", "1")] {
        set_env(&url, &[]);
        assert_eq!(
            call(&app, &editor, "POST", CREATE, hero.clone(), &[header]).await,
            StatusCode::CREATED
        );
        flush().await;
        assert!(payloads(&received).is_empty(), "{header:?} must drop it");
    }

    // ... unless the instance turned GPC handling off.
    set_env(&url, &[("ANALYTICS_RESPECT_GPC", "false")]);
    call(
        &app,
        &editor,
        "POST",
        CREATE,
        hero.clone(),
        &[("Sec-GPC", "1")],
    )
    .await;
    flush().await;
    assert_eq!(payloads(&received).len(), 1);

    // Opt-in: nothing without the consent header (or with another value or
    // a differently named header), the event with `Analytics-Consent: 1`.
    for headers in [
        vec![],
        vec![("Analytics-Consent", "0")],
        vec![("X-Analytics-Consent", "1")],
    ] {
        set_env(&url, &[("ANALYTICS_CONSENT", "opt-in")]);
        call(&app, &editor, "POST", CREATE, hero.clone(), &headers).await;
        flush().await;
        assert!(payloads(&received).is_empty(), "{headers:?}");
    }
    set_env(&url, &[("ANALYTICS_CONSENT", "opt-in")]);
    call(
        &app,
        &editor,
        "POST",
        CREATE,
        hero.clone(),
        &[("Analytics-Consent", "1")],
    )
    .await;
    flush().await;
    assert_eq!(payloads(&received).len(), 1);

    // Opt-in plus consent still loses to GPC.
    set_env(&url, &[("ANALYTICS_CONSENT", "opt-in")]);
    call(
        &app,
        &editor,
        "POST",
        CREATE,
        hero.clone(),
        &[("Analytics-Consent", "1"), ("Sec-GPC", "1")],
    )
    .await;
    flush().await;
    assert!(payloads(&received).is_empty());

    // A write that did not commit emits nothing: a body failing validation
    // (422) and a caller without the write role (403).
    set_env(&url, &[]);
    assert_eq!(
        call(
            &app,
            &editor,
            "POST",
            CREATE,
            json!({"name": "", "powers": []}),
            &[]
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let viewer = token(&app, &["viewer"]).await;
    assert_eq!(
        call(&app, &viewer, "POST", CREATE, hero.clone(), &[]).await,
        StatusCode::FORBIDDEN
    );
    flush().await;
    assert!(payloads(&received).is_empty());

    // The allowlist: a key off `ANALYTICS_PROP_KEYS` is dropped, so the
    // event carries no data at all.
    set_env(&url, &[("ANALYTICS_PROP_KEYS", "something_else")]);
    call(&app, &editor, "POST", CREATE, hero.clone(), &[]).await;
    flush().await;
    let events = payloads(&received);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["name"], "hero.created");
    assert!(events[0].get("data").is_none(), "{events:?}");

    // Disabled: nothing is sent.
    set_env(&url, &[("ANALYTICS_ENABLED", "false")]);
    call(&app, &editor, "POST", CREATE, hero.clone(), &[]).await;
    flush().await;
    assert!(payloads(&received).is_empty());

    // The shared test pin (`common::disable_analytics`): with the
    // environment still selecting the stand-in, a create and a delete
    // through the router reach nothing.
    set_env(&url, &[("ANALYTICS_ENABLED", "true")]);
    common::disable_analytics();
    assert_eq!(
        call(&app, &editor, "POST", CREATE, hero.clone(), &[]).await,
        StatusCode::CREATED
    );
    assert_eq!(
        call(
            &app,
            &maintainer,
            "DELETE",
            &format!("{CREATE}?id=2"),
            Value::Null,
            &[]
        )
        .await,
        StatusCode::NO_CONTENT
    );
    flush().await;
    assert!(
        payloads(&received).is_empty(),
        "the pinned client must not post to the stand-in"
    );

    // Control: once the pin is flushed away, the same call reaches it again.
    set_env(&url, &[("ANALYTICS_ENABLED", "true")]);
    call(&app, &editor, "POST", CREATE, hero, &[]).await;
    flush().await;
    assert_eq!(payloads(&received).len(), 1);
}
