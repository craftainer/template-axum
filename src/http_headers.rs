//! RFC 8594 `Sunset`/`Deprecation` response headers and the baseline
//! security response headers -- port of `app/http_headers.py`'s
//! `sunset()` and `add_security_headers()` (`docs/adrs/0030` for the
//! latter; `docs/nfrs/NFR-0002-deprecation-sunset-headers.md` for the
//! former).
//!
//! Applied by every route in `controllers::heroes_v1`/`heroes_v1_xml`
//! (NFR-0026, `docs/adrs/0012`, `docs/adrs/0017`) -- the deprecated Hero
//! v1 compat routers this mechanism was built ahead of.

use axum::http::header::{HeaderValue, LINK};
use axum::http::HeaderName;
use axum::response::{IntoResponseParts, Response, ResponseParts};
use chrono::{DateTime, Utc};

/// Every served HTML page loads its only script from the same origin
/// (`<script src="{own_base}/components.js">`, never inline) and defines
/// no other resource type, so a locked-down `default-src` covers it
/// without an `unsafe-inline`/`unsafe-eval` carve-out. `frame-ancestors
/// 'none'` is CSP's own `X-Frame-Options: DENY`, set alongside it below
/// for the older header's wider browser support. The same policy
/// template-fastapi serves (it relaxes it only for FastAPI's own
/// `/docs` and `/redoc`, which this app has no equivalent of).
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; frame-ancestors 'none'";

/// Headers `security_headers` sets on every response, in the exact
/// values template-fastapi's middleware uses.
const SECURITY_HEADERS: [(&str, &str); 6] = [
    ("content-security-policy", CONTENT_SECURITY_POLICY),
    ("x-frame-options", "DENY"),
    ("x-content-type-options", "nosniff"),
    // Pins every origin serving this app to HTTPS for future requests;
    // browsers ignore it over plain HTTP, so it is harmless in dev.
    (
        "strict-transport-security",
        "max-age=63072000; includeSubDomains",
    ),
    ("referrer-policy", "no-referrer"),
    (
        "permissions-policy",
        "geolocation=(), microphone=(), camera=()",
    ),
];

/// Response layer (`axum::middleware::map_response`) that sets
/// `SECURITY_HEADERS` on every response -- error responses and the
/// `/.well-known/` documents included, which no route-level opt-in would
/// reach. Overwrites a header a handler already set, like the reference.
pub async fn security_headers(mut response: Response) -> Response {
    for (name, value) in SECURITY_HEADERS {
        response.headers_mut().insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    response
}

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

    #[tokio::test]
    async fn security_headers_cover_every_response_and_overwrite_a_handlers_own() {
        let mut response = (axum::http::StatusCode::NOT_FOUND, "nope").into_response();
        response
            .headers_mut()
            .insert("x-frame-options", HeaderValue::from_static("SAMEORIGIN"));
        let response = security_headers(response).await;
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .unwrap()
                .to_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(
            header("content-security-policy"),
            "default-src 'self'; frame-ancestors 'none'"
        );
        assert_eq!(header("x-frame-options"), "DENY");
        assert_eq!(header("x-content-type-options"), "nosniff");
        assert_eq!(
            header("strict-transport-security"),
            "max-age=63072000; includeSubDomains"
        );
        assert_eq!(header("referrer-policy"), "no-referrer");
        assert_eq!(
            header("permissions-policy"),
            "geolocation=(), microphone=(), camera=()"
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
