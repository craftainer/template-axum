//! RFC 9457 Problem Details error responses -- port of `problem_details.py`.
//! `AppError` is the single error type every controller returns; its
//! `IntoResponse` impl is the one place that renders
//! `application/problem+json`, matching `register_problem_handlers(app)`
//! being called once from `main.py`.

use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::config::Mode;
use craftainer_crud::error::{redacted_detail, CrudError};
use craftainer_crud::repositories::RepoError;
use craftainer_crud::views::FieldError;

/// RFC 9457 body shape -- `type`/`title`/`status`/`detail`/`instance`.
#[derive(Debug, Serialize)]
struct ProblemDetail {
    #[serde(rename = "type")]
    kind: &'static str,
    title: &'static str,
    status: u16,
    detail: ProblemDetail_Detail,
}

// serde_json can't serialize a bare enum untagged as "either a string or a
// list of field errors" without a small wrapper -- this is that wrapper,
// matching `problem_details.py`'s `detail: str | list`.
#[derive(Debug, Serialize)]
#[serde(untagged)]
#[allow(non_camel_case_types)]
enum ProblemDetail_Detail {
    Text(String),
    Fields(Vec<FieldError>),
}

/// The one error type every controller/extractor returns. Deliberately
/// narrow (mirrors the handful of `HTTPException`s the Python routers
/// actually raise) rather than a per-resource error enum.
#[derive(Debug)]
pub enum AppError {
    NotFound(String),
    /// A request that carried no usable bearer credentials at all. Its 401
    /// gets the bare RFC 6750 challenge (section 3.1).
    Unauthorized(String),
    /// A bearer token that was presented and rejected (bad signature,
    /// issuer, audience or expiry). Its 401 adds `error="invalid_token"`.
    InvalidToken(String),
    Forbidden(String),
    UnprocessableEntity(Vec<FieldError>),
    /// An `If-Match` precondition failed (412). Produced by
    /// `CrudError::PreconditionFailed`; returned by a stale `If-Match` on
    /// a single-record Hero `PATCH`/`DELETE` (`docs/adrs/0023`).
    PreconditionFailed(String),
    ServiceUnavailable(String),
    TooManyRequests(String),
    Internal(String),
}

/// Set once at startup (`main.rs`) so `AppError::Internal` and
/// `CrudError::Internal` redact their detail outside `Mode::Dev` (NFR-0015)
/// without threading `Settings` through every controller. The switch itself
/// lives in `craftainer-crud` (one static for both error types).
pub fn configure_detail_redaction(mode: Mode) {
    craftainer_crud::error::configure_detail_redaction(mode != Mode::Dev);
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let rejected_token = matches!(self, AppError::InvalidToken(_));
        let (status, title, detail): (StatusCode, &'static str, ProblemDetail_Detail) = match self {
            AppError::NotFound(msg) => (
                StatusCode::NOT_FOUND,
                "Not Found",
                ProblemDetail_Detail::Text(msg),
            ),
            AppError::Unauthorized(msg) => (
                StatusCode::UNAUTHORIZED,
                "Unauthorized",
                ProblemDetail_Detail::Text(msg),
            ),
            AppError::InvalidToken(msg) => (
                StatusCode::UNAUTHORIZED,
                "Unauthorized",
                ProblemDetail_Detail::Text(msg),
            ),
            AppError::Forbidden(msg) => (
                StatusCode::FORBIDDEN,
                "Forbidden",
                ProblemDetail_Detail::Text(msg),
            ),
            AppError::UnprocessableEntity(errors) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Unprocessable Content",
                ProblemDetail_Detail::Fields(errors),
            ),
            AppError::PreconditionFailed(msg) => (
                StatusCode::PRECONDITION_FAILED,
                "Precondition Failed",
                ProblemDetail_Detail::Text(msg),
            ),
            AppError::ServiceUnavailable(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Service Unavailable",
                ProblemDetail_Detail::Text(msg),
            ),
            AppError::TooManyRequests(msg) => (
                StatusCode::TOO_MANY_REQUESTS,
                "Too Many Requests",
                ProblemDetail_Detail::Text(msg),
            ),
            AppError::Internal(msg) => {
                tracing::error!(error = %msg, "unhandled internal error");
                let detail = redacted_detail(msg, "Internal Server Error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal Server Error",
                    ProblemDetail_Detail::Text(detail),
                )
            }
        };

        let body = ProblemDetail {
            kind: "about:blank",
            title,
            status: status.as_u16(),
            detail,
        };
        let mut response = (status, axum::Json(body)).into_response();
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            "application/problem+json".parse().unwrap(),
        );
        if status == StatusCode::UNAUTHORIZED {
            // Without a configured `resource_metadata` URL (which only
            // `add_resource_metadata` below knows); the router layer
            // replaces this with the full challenge.
            response.headers_mut().insert(
                axum::http::header::WWW_AUTHENTICATE,
                HeaderValue::from_static(if rejected_token {
                    "Bearer error=\"invalid_token\""
                } else {
                    "Bearer"
                }),
            );
            if rejected_token {
                response.extensions_mut().insert(RejectedToken);
            }
        }
        response
    }
}

/// Response extension `AppError::InvalidToken` sets, so the router layer
/// can add `error="invalid_token"` to the challenge it builds.
#[derive(Debug, Clone, Copy)]
struct RejectedToken;

/// The RFC 9728 challenge for a 401: `Bearer resource_metadata="<url>"`,
/// plus `error="invalid_token"` for a rejected token. `url` comes from
/// configuration, never from the request.
fn bearer_challenge(resource_metadata_url: &str, rejected_token: bool) -> Option<HeaderValue> {
    let mut challenge = format!("Bearer resource_metadata=\"{resource_metadata_url}\"");
    if rejected_token {
        challenge.push_str(", error=\"invalid_token\"");
    }
    HeaderValue::from_str(&challenge).ok()
}

/// Router layer (`axum::middleware::map_response_with_state`): points every
/// 401's `WWW-Authenticate` at the RFC 9728 metadata document. Applied to
/// the whole router, so a 401 from any source is covered, not only the
/// ones `AppError` builds.
pub async fn add_resource_metadata(
    axum::extract::State(resource_metadata_url): axum::extract::State<std::sync::Arc<str>>,
    mut response: Response,
) -> Response {
    if response.status() == StatusCode::UNAUTHORIZED {
        let rejected = response.extensions().get::<RejectedToken>().is_some();
        if let Some(challenge) = bearer_challenge(&resource_metadata_url, rejected) {
            response
                .headers_mut()
                .insert(axum::http::header::WWW_AUTHENTICATE, challenge);
        }
    }
    response
}

impl From<CrudError> for AppError {
    fn from(err: CrudError) -> Self {
        match err {
            CrudError::NotFound(msg) => AppError::NotFound(msg),
            CrudError::UnprocessableEntity(errors) => AppError::UnprocessableEntity(errors),
            CrudError::PreconditionFailed(msg) => AppError::PreconditionFailed(msg),
            CrudError::ServiceUnavailable(msg) => AppError::ServiceUnavailable(msg),
            CrudError::TooManyRequests(msg) => AppError::TooManyRequests(msg),
            CrudError::Internal(msg) => AppError::Internal(msg),
            // `CrudError` is non-exhaustive: an unknown future variant is a
            // server-side problem, never a client error.
            other => AppError::Internal(other.to_string()),
        }
    }
}

impl From<RepoError> for AppError {
    fn from(err: RepoError) -> Self {
        AppError::Internal(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use std::sync::Mutex;

    // configure_detail_redaction flips a process-wide static -- serialize
    // any test that touches it, same pattern as config.rs's ENV_LOCK.
    static REDACTION_LOCK: Mutex<()> = Mutex::new(());

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn not_found_maps_to_404_with_problem_json_content_type() {
        let response = AppError::NotFound("hero 1 not found".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .unwrap(),
            "application/problem+json"
        );
        let body = body_json(response).await;
        assert_eq!(body["title"], "Not Found");
        assert_eq!(body["status"], 404);
        assert_eq!(body["detail"], "hero 1 not found");
        assert_eq!(body["type"], "about:blank");
    }

    #[tokio::test]
    async fn unauthorized_maps_to_401_and_sets_www_authenticate() {
        let response = AppError::Unauthorized("bad token".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::WWW_AUTHENTICATE)
                .unwrap(),
            "Bearer"
        );
    }

    #[tokio::test]
    async fn invalid_token_is_a_401_that_names_the_error() {
        let response = AppError::InvalidToken("bad".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get("www-authenticate").unwrap(),
            "Bearer error=\"invalid_token\""
        );
        let body = body_json(response).await;
        assert_eq!(body["title"], "Unauthorized");
        assert_eq!(body["detail"], "bad");
    }

    async fn with_challenge(response: Response) -> Response {
        add_resource_metadata(
            axum::extract::State(std::sync::Arc::from("https://api.example/m")),
            response,
        )
        .await
    }

    #[tokio::test]
    async fn the_layer_adds_the_configured_resource_metadata_to_every_401() {
        let missing = with_challenge(AppError::Unauthorized("x".to_string()).into_response()).await;
        assert_eq!(
            missing.headers().get("www-authenticate").unwrap(),
            "Bearer resource_metadata=\"https://api.example/m\""
        );
        let rejected =
            with_challenge(AppError::InvalidToken("x".to_string()).into_response()).await;
        assert_eq!(
            rejected.headers().get("www-authenticate").unwrap(),
            "Bearer resource_metadata=\"https://api.example/m\", error=\"invalid_token\""
        );
        // A 401 nobody built with AppError (no header at all) gets one too.
        let bare = with_challenge(StatusCode::UNAUTHORIZED.into_response()).await;
        assert_eq!(
            bare.headers().get("www-authenticate").unwrap(),
            "Bearer resource_metadata=\"https://api.example/m\""
        );
    }

    #[tokio::test]
    async fn the_layer_leaves_other_statuses_alone() {
        let forbidden = with_challenge(AppError::Forbidden("x".to_string()).into_response()).await;
        assert!(forbidden.headers().get("www-authenticate").is_none());
    }

    #[test]
    fn a_challenge_that_cannot_be_a_header_value_is_dropped() {
        assert!(bearer_challenge("https://api.example/\n", false).is_none());
    }

    #[tokio::test]
    async fn forbidden_maps_to_403_without_www_authenticate() {
        let response = AppError::Forbidden("Insufficient role".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(response
            .headers()
            .get(axum::http::header::WWW_AUTHENTICATE)
            .is_none());
    }

    #[tokio::test]
    async fn unprocessable_entity_serializes_field_errors_as_a_list() {
        let response = AppError::UnprocessableEntity(vec![
            FieldError::new("name", "must be 1-200 characters"),
            FieldError::new("powers", "must contain at least one power"),
        ])
        .into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = body_json(response).await;
        let detail = body["detail"].as_array().unwrap();
        assert_eq!(detail.len(), 2);
        assert_eq!(detail[0]["field"], "name");
        assert_eq!(detail[1]["field"], "powers");
    }

    #[tokio::test]
    async fn service_unavailable_maps_to_503() {
        let response =
            AppError::ServiceUnavailable("Authentication service unavailable".to_string())
                .into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn too_many_requests_maps_to_429() {
        let response = AppError::TooManyRequests("rate limit exceeded".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let body = body_json(response).await;
        assert_eq!(body["detail"], "rate limit exceeded");
    }

    #[tokio::test]
    async fn internal_error_detail_is_redacted_outside_dev_mode() {
        // into_response() is where redaction actually happens (it reads
        // REDACT_INTERNAL_DETAIL synchronously) -- the guard only needs to
        // span that call, not the later `.await` on the body, so it never
        // crosses an await point (clippy::await_holding_lock).
        let response = {
            let _guard = REDACTION_LOCK.lock().unwrap();
            configure_detail_redaction(Mode::Production);
            let response = AppError::Internal("db pool exhausted: secret-ish detail".to_string())
                .into_response();
            configure_detail_redaction(Mode::Dev);
            response
        };
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_json(response).await;
        assert_eq!(body["detail"], "Internal Server Error");
    }

    #[tokio::test]
    async fn internal_error_detail_is_shown_in_dev_mode() {
        let response = {
            let _guard = REDACTION_LOCK.lock().unwrap();
            configure_detail_redaction(Mode::Dev);
            AppError::Internal("db pool exhausted".to_string()).into_response()
        };
        let body = body_json(response).await;
        assert_eq!(body["detail"], "db pool exhausted");
    }

    #[test]
    fn repo_error_converts_to_an_internal_app_error() {
        let repo_err = RepoError::Backend("connection reset".to_string());
        let app_err: AppError = repo_err.into();
        assert!(matches!(app_err, AppError::Internal(msg) if msg.contains("connection reset")));
    }

    #[tokio::test]
    async fn precondition_failed_maps_to_412() {
        let response = AppError::PreconditionFailed("stale".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
        let body = body_json(response).await;
        assert_eq!(body["title"], "Precondition Failed");
        assert_eq!(body["detail"], "stale");
    }

    #[test]
    fn every_crud_error_converts_to_its_app_error_twin() {
        assert!(matches!(
            AppError::from(CrudError::NotFound("a".into())),
            AppError::NotFound(m) if m == "a"
        ));
        assert!(matches!(
            AppError::from(CrudError::UnprocessableEntity(vec![FieldError::new("f", "m")])),
            AppError::UnprocessableEntity(e) if e.len() == 1
        ));
        assert!(matches!(
            AppError::from(CrudError::PreconditionFailed("p".into())),
            AppError::PreconditionFailed(m) if m == "p"
        ));
        assert!(matches!(
            AppError::from(CrudError::ServiceUnavailable("s".into())),
            AppError::ServiceUnavailable(m) if m == "s"
        ));
        assert!(matches!(
            AppError::from(CrudError::TooManyRequests("t".into())),
            AppError::TooManyRequests(m) if m == "t"
        ));
        assert!(matches!(
            AppError::from(CrudError::Internal("i".into())),
            AppError::Internal(m) if m == "i"
        ));
    }
}
