//! RFC 8594 `Sunset`/`Deprecation` response headers -- port of the
//! `sunset()` half of `app/http_headers.py` (the security-headers
//! middleware half of that module is out of scope for this item; see
//! `docs/nfrs/NFR-0002-deprecation-sunset-headers.md`'s port below and
//! `docs/plans/2026-09-tier-b-c-app-features.md`).
//!
//! Built now with no deprecated route to apply it to yet -- the Hero v1
//! compat routes that would use it are Tier C item 7, explicitly out of
//! scope for this plan (see the plan's own item 7 and `docs/adrs/0002`/
//! `0009`'s note that this app hasn't established an API-versioning
//! pattern yet). `Sunset` is exercised directly by this module's own tests
//! in the meantime -- `#![allow(dead_code)]` below is temporary, not a
//! permanent exemption, until item 7 (or an ADR revising this plan) gives
//! it a real call site.
#![allow(dead_code)]

use axum::http::header::{HeaderValue, LINK};
use axum::http::HeaderName;
use axum::response::{IntoResponseParts, ResponseParts};
use chrono::{DateTime, Utc};

static DEPRECATION: HeaderName = HeaderName::from_static("deprecation");
static SUNSET: HeaderName = HeaderName::from_static("sunset");

/// Combine into any handler's return type via a tuple (e.g. `(Sunset::new(at,
/// Some(link)), Json(body))`) to set `Deprecation: true` and a `Sunset`
/// HTTP-date on that response alone -- mirrors `sunset()` being a per-route
/// opt-in FastAPI dependency in the reference, not global middleware
/// (`http_headers.py`'s own docstring: every served page there loads
/// per-route, so a global `Sunset` header would be wrong on current-version
/// routes, which `NFR-0002` requires stay header-free).
pub struct Sunset {
    at: DateTime<Utc>,
    link: Option<&'static str>,
}

impl Sunset {
    /// `at` is rendered as an RFC 7231 HTTP-date (`Sun, 06 Nov 1994
    /// 08:49:37 GMT`), matching RFC 8594's required `Sunset` format --
    /// not an RFC 3339/ISO 8601 timestamp. `link`, when given, is rendered
    /// as `Link: <link>; rel="sunset"`.
    pub fn new(at: DateTime<Utc>, link: Option<&'static str>) -> Self {
        Self { at, link }
    }
}

impl IntoResponseParts for Sunset {
    type Error = std::convert::Infallible;

    fn into_response_parts(self, mut res: ResponseParts) -> Result<ResponseParts, Self::Error> {
        res.headers_mut()
            .insert(DEPRECATION.clone(), HeaderValue::from_static("true"));
        // RFC 7231's HTTP-date format is always plain ASCII, so this never
        // fails -- unlike `link` below, nothing here is caller-supplied.
        let http_date = self.at.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        res.headers_mut().insert(
            SUNSET.clone(),
            HeaderValue::from_str(&http_date).expect("HTTP-date is always valid ASCII"),
        );
        if let Some(link) = self.link {
            res.headers_mut().insert(
                LINK,
                HeaderValue::from_str(&format!("<{link}>; rel=\"sunset\""))
                    .expect("every call site passes a header-safe path, not arbitrary input"),
            );
        }
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use chrono::TimeZone;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap()
    }

    #[test]
    fn sets_deprecation_true_and_a_sunset_http_date() {
        let response = (Sunset::new(at(), None), "body").into_response();
        assert_eq!(response.headers().get("deprecation").unwrap(), "true");
        assert_eq!(
            response.headers().get("sunset").unwrap(),
            "Fri, 01 Jan 2027 00:00:00 GMT"
        );
    }

    #[test]
    fn omits_link_when_none() {
        let response = (Sunset::new(at(), None), "body").into_response();
        assert!(response.headers().get(LINK).is_none());
    }

    #[test]
    fn sets_link_with_sunset_rel_when_given() {
        let response = (Sunset::new(at(), Some("/crud/v1/heroes/v2/json")), "body").into_response();
        assert_eq!(
            response.headers().get(LINK).unwrap(),
            "</crud/v1/heroes/v2/json>; rel=\"sunset\""
        );
    }
}
