//! Product analytics wiring (`docs/adrs/0029`, FR-0037, NFR-0033): the
//! per-request GPC/DNT and consent signals for `track()`, and Hero's sample
//! domain events.
//!
//! `craftainer_observability::analytics` (feature `analytics`) does the
//! work: it validates an event (name pattern, allowlisted scalar props,
//! path-only url), reads `ANALYTICS_*` from the environment itself
//! (`ANALYTICS_ENABLED`, `_BACKEND`, `_ENDPOINT`, `_SITE_ID`, `_HOSTNAME`,
//! `_CONSENT`, `_RESPECT_GPC`, `_GEO`, `_PROP_KEYS`) and delivers through a
//! bounded, non-blocking queue. `config.rs` therefore carries no
//! `ANALYTICS_*` fields: a second copy would be a second source of truth.
//!
//! What the library cannot know is what a request says about tracking.
//! [`analytics_context`] reads that from the headers and runs the request
//! inside the library's `RequestContext`:
//!
//! - `Sec-GPC: 1` (Global Privacy Control) or `DNT: 1` (Do Not Track) sets
//!   `gpc`; the library then drops every event of the request while
//!   `ANALYTICS_RESPECT_GPC` is true (the default).
//! - `Analytics-Consent: 1` sets `consent`. It matters only with
//!   `ANALYTICS_CONSENT=opt-in`, where no event is recorded for a request
//!   without it. A browser never sends this header by itself: a front end
//!   or BFF that has collected consent adds it to its calls.
//!
//! `build_router` applies the layer to the whole router, so no handler can
//! call `track()` outside a request context and skip the GPC/consent
//! signals.
//!
//! The context is a thread-local set around every poll of the wrapped
//! future, so handlers see it. It does not follow into a `tokio::spawn`ed
//! task (which polls on another thread, outside the scope): a handler that
//! tracks from a spawned task must wrap that future itself, with
//! `current_request_context()` captured before the spawn and
//! `RequestContext::scope` around the spawned future; otherwise the task's
//! events are recorded without the request's GPC/consent signals. None of
//! the events here are tracked from a spawned task.
//!
//! The events ([`hero_created`], [`hero_deleted`]) are called by the v2 JSON
//! handlers after the write succeeded, so a failed (rolled-back) write emits
//! nothing. `url` is the constant route path, never the raw path with ids,
//! and the only property is `powers_count`, a count: no names, ids or owners.

use axum::extract::Request;
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;
use craftainer_observability::analytics::{track, RequestContext};

use crate::hero::models::hero;

/// The header a front end or BFF sets to `1` once the visitor consented
/// (`ANALYTICS_CONSENT=opt-in`). Exactly this name; backends read only it.
pub const CONSENT_HEADER: &str = "Analytics-Consent";

/// The route path every Hero event is attributed to.
pub const HERO_URL: &str = "/crud/v1/heroes/v2/json";

/// Whether header `name` is present with the value `1` (the only value that
/// counts).
fn signalled(headers: &HeaderMap, name: &str) -> bool {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim() == "1")
}

/// What `headers` say about tracking: GPC (`Sec-GPC: 1` or `DNT: 1`) and
/// consent (`Analytics-Consent: 1`).
pub fn request_context(headers: &HeaderMap) -> RequestContext {
    RequestContext {
        gpc: signalled(headers, "Sec-GPC") || signalled(headers, "DNT"),
        consent: signalled(headers, CONSENT_HEADER),
    }
}

/// axum middleware (`axum::middleware::from_fn`): runs each request inside
/// its [`request_context`].
pub async fn analytics_context(request: Request, next: Next) -> Response {
    request_context(request.headers())
        .scope(next.run(request))
        .await
}

/// `hero.created`, with `powers_count` (how many powers the new hero has).
pub fn hero_created(hero: &hero::Model) {
    let powers = hero.powers.as_ref().map_or(0, Vec::len);
    track(
        "hero.created",
        &[("powers_count", u64::try_from(powers).unwrap_or(0).into())],
        Some(HERO_URL),
    );
}

/// `hero.deleted`, no properties.
pub fn hero_deleted() {
    track("hero.deleted", &[], Some(HERO_URL));
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn gpc_and_dnt_each_set_gpc_and_only_the_value_one_counts() {
        assert!(request_context(&headers(&[("sec-gpc", "1")])).gpc);
        assert!(request_context(&headers(&[("dnt", "1")])).gpc);
        assert!(request_context(&headers(&[("dnt", " 1 ")])).gpc);
        assert!(!request_context(&headers(&[("sec-gpc", "0")])).gpc);
        assert!(!request_context(&headers(&[("dnt", "yes")])).gpc);
        assert!(!request_context(&HeaderMap::new()).gpc);
    }

    #[test]
    fn consent_needs_the_exact_header_with_the_value_one() {
        assert!(request_context(&headers(&[("analytics-consent", "1")])).consent);
        assert!(!request_context(&headers(&[("analytics-consent", "true")])).consent);
        assert!(!request_context(&headers(&[("x-analytics-consent", "1")])).consent);
        assert!(!request_context(&HeaderMap::new()).consent);
    }

    #[test]
    fn a_non_ascii_header_value_is_not_a_signal() {
        let mut map = HeaderMap::new();
        map.insert("dnt", HeaderValue::from_bytes(&[0xE9]).unwrap());
        assert!(!request_context(&map).gpc);
    }
}
