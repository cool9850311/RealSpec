//! The headers every response carries, success or failure alike
//! (`openapi.yaml`: "Every response carries `X-Request-Id`, `Cache-Control:
//! no-store` and `X-Content-Type-Options: nosniff`"), applied as one layer
//! around the whole router so no individual handler has to remember them —
//! including the ones that answer through an `IntoResponse` rejection before
//! any handler body runs at all.

use axum::extract::{Request, State};
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;
use uuid::Uuid;

use crate::state::AppState;

const REQUEST_ID_HEADER: &str = "x-request-id";

/// The pure half of request-id handling: echo a well-formed caller-supplied
/// id verbatim, or mint a fresh one — split out so it is unit-testable
/// without standing up a router (`spec.md`, "Test plan" -> `api`: "request id
/// handling"). `openapi.yaml`'s own `X-Request-Id` parameter caps it at 128
/// characters; an empty header is treated the same as an absent one.
fn resolve_request_id(incoming: Option<&str>) -> String {
    match incoming {
        Some(s) if !s.is_empty() && s.len() <= 128 => s.to_string(),
        _ => format!("req_{}", Uuid::new_v4().simple()),
    }
}

/// A caller's own id is echoed back verbatim if present (`observability.feature`:
/// "on success, and on refusal alike"); otherwise a fresh one is minted with
/// the `req_` prefix that same feature asserts.
pub async fn request_context(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let incoming = req
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok());
    let request_id = resolve_request_id(incoming);

    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let span = tracing::info_span!(
        "request",
        request_id = %request_id,
        method = %method,
        path = %path,
        instance_id = %state.common.instance_id,
    );

    async move {
        let mut response = next.run(req).await;

        if let Ok(value) = HeaderValue::from_str(&request_id) {
            response.headers_mut().insert("X-Request-Id", value);
        }
        response
            .headers_mut()
            .insert("Cache-Control", HeaderValue::from_static("no-store"));
        response.headers_mut().insert(
            "X-Content-Type-Options",
            HeaderValue::from_static("nosniff"),
        );

        tracing::info!(status = response.status().as_u16(), "request completed");
        response
    }
    .instrument(span)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_caller_supplied_id_is_echoed_back_verbatim() {
        assert_eq!(
            resolve_request_id(Some("merchant-trace-7f3a")),
            "merchant-trace-7f3a"
        );
    }

    #[test]
    fn a_missing_id_is_minted_fresh_with_the_documented_prefix() {
        let id = resolve_request_id(None);
        assert!(id.starts_with("req_"), "{id:?} should start with req_");
    }

    #[test]
    fn an_empty_id_is_treated_as_though_none_was_sent() {
        let id = resolve_request_id(Some(""));
        assert!(id.starts_with("req_"));
    }

    #[test]
    fn an_id_past_the_128_character_limit_is_replaced_rather_than_echoed() {
        let too_long = "x".repeat(129);
        let id = resolve_request_id(Some(&too_long));
        assert!(id.starts_with("req_"));
    }

    #[test]
    fn an_id_at_exactly_the_128_character_limit_is_still_echoed() {
        let boundary = "x".repeat(128);
        assert_eq!(resolve_request_id(Some(&boundary)), boundary);
    }

    #[test]
    fn two_minted_ids_are_not_the_same() {
        assert_ne!(resolve_request_id(None), resolve_request_id(None));
    }
}
