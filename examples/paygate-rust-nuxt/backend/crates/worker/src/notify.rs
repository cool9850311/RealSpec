//! The notify role: polls [`paygate_infra::notifier::run_one`], supplying the
//! one piece it deliberately does not own — the actual HTTP delivery
//! (`crates/infra/src/notifier.rs`'s own doc comment: "You supply the HTTP
//! delivery"): `reqwest` with **redirects disabled** (`spec.md`, "Telling
//! the merchant": "the notifier does not follow redirects"),
//! `NOTIFY_TIMEOUT_MS`, and a payment's path-only `notify_url` resolved
//! against `MERCHANT_BASE_URL` (`spec.md`, "Environment variables").

use std::time::Duration;

use tokio::sync::watch;

use paygate_infra::config::NotifyConfig;
use paygate_infra::db::Db;
#[cfg(test)]
use paygate_infra::notifier::DueNotification;
use paygate_infra::notifier::{self, DeliveryResult};

/// Delivers one already-resolved request as
/// `application/x-www-form-urlencoded`, and reports exactly what came back,
/// never guessing: no answer at all (a timeout, a connection error) is
/// `DeliveryResult { status: None, body: None }`, the same failed bucket
/// `spec.md` puts "not answering" in.
///
/// Takes owned `url`/`body` rather than borrowing a `DueNotification`
/// directly, so the future this returns owns everything it touches — the
/// closure `poll_loop` hands to [`notifier::run_one`] is a
/// `for<'a> FnOnce(&'a DueNotification) -> Fut`, and `Fut` cannot depend on
/// `'a` (a plain lifetime limitation of that shape, not a borrow-checker
/// bug), so every field this needs from the claimed row has to be copied out
/// before the `async` block starts, not read from inside it.
async fn deliver_owned(
    http: reqwest::Client,
    url: String,
    timeout: Duration,
    body: String,
) -> DeliveryResult {
    let result = http
        .post(&url)
        .timeout(timeout)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body)
        .send()
        .await;

    match result {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.text().await.ok();
            DeliveryResult {
                status: Some(status),
                body,
            }
        }
        Err(_) => DeliveryResult {
            status: None,
            body: None,
        },
    }
}

/// The convenience form used directly by the unit tests below: something with
/// a concrete `DueNotification` in hand, that does not need to satisfy
/// [`notifier::run_one`]'s higher-ranked closure signature the way
/// `poll_loop` does.
#[cfg(test)]
async fn deliver(
    http: &reqwest::Client,
    merchant_base_url: &str,
    timeout: Duration,
    due: &DueNotification,
) -> DeliveryResult {
    let url = format!("{merchant_base_url}{}", due.url);
    let body = notifier::payload_to_form_body(&due.payload);
    deliver_owned(http.clone(), url, timeout, body).await
}

/// Polls forever: claim-and-deliver one due row, and only sleep when there
/// was nothing due, or the claim itself failed — a row still due after a
/// successful delivery is picked up again immediately rather than waiting out
/// `NOTIFY_POLL_INTERVAL_MS` for no reason.
pub async fn poll_loop(
    db: Db,
    http: reqwest::Client,
    cfg: NotifyConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if crate::shutdown::requested(&shutdown) {
            return;
        }
        let outcome = notifier::run_one(&db, cfg.max_attempts, &cfg.backoff_ms, |due| {
            let http = http.clone();
            let url = format!("{}{}", cfg.merchant_base_url, due.url);
            let body = notifier::payload_to_form_body(&due.payload);
            let timeout = cfg.timeout;
            deliver_owned(http, url, timeout, body)
        })
        .await;

        match outcome {
            Ok(Some(_)) => continue,
            Ok(None) => crate::shutdown::sleep_or_shutdown(cfg.poll_interval, &mut shutdown).await,
            Err(err) => {
                tracing::error!(error = %err, "notifier pass failed");
                crate::shutdown::sleep_or_shutdown(cfg.poll_interval, &mut shutdown).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Response, StatusCode};
    use axum::routing::post;
    use axum::Router;
    use uuid::Uuid;

    async fn spawn(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    fn http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    fn due(path: &str) -> DueNotification {
        DueNotification {
            id: 1,
            payment_id: Uuid::from_u128(1),
            merchant_id: 1,
            url: path.to_string(),
            payload: serde_json::json!({"MerchantTradeNo": "ACME-1", "RtnCode": "1"}),
            attempts: 0,
        }
    }

    #[tokio::test]
    async fn an_exact_1ok_body_is_delivered() {
        let app = Router::new().route("/notify", post(|| async { (StatusCode::OK, "1|OK") }));
        let base = spawn(app).await;
        let result = deliver(
            &http_client(),
            &base,
            Duration::from_secs(2),
            &due("/notify"),
        )
        .await;
        assert!(result.delivered());
        assert_eq!(result.status, Some(200));
    }

    #[tokio::test]
    async fn the_wrong_body_with_200_is_not_delivered() {
        let app = Router::new().route("/notify", post(|| async { (StatusCode::OK, "0|ERROR") }));
        let base = spawn(app).await;
        let result = deliver(
            &http_client(),
            &base,
            Duration::from_secs(2),
            &due("/notify"),
        )
        .await;
        assert!(!result.delivered());
    }

    #[tokio::test]
    async fn a_1ok_body_with_a_trailing_newline_is_not_delivered() {
        let app = Router::new().route("/notify", post(|| async { (StatusCode::OK, "1|OK\n") }));
        let base = spawn(app).await;
        let result = deliver(
            &http_client(),
            &base,
            Duration::from_secs(2),
            &due("/notify"),
        )
        .await;
        assert!(!result.delivered());
    }

    #[tokio::test]
    async fn a_redirect_is_not_followed_and_is_a_failed_delivery() {
        async fn redirect() -> Response<Body> {
            Response::builder()
                .status(StatusCode::FOUND)
                .header("location", "/somewhere-else")
                .body(Body::empty())
                .unwrap()
        }
        let app = Router::new().route("/notify", post(redirect));
        let base = spawn(app).await;
        let result = deliver(
            &http_client(),
            &base,
            Duration::from_secs(2),
            &due("/notify"),
        )
        .await;
        assert!(!result.delivered());
        assert_eq!(result.status, Some(302));
    }

    #[tokio::test]
    async fn a_response_slower_than_the_timeout_is_a_failed_delivery_with_no_status() {
        async fn slow() -> (StatusCode, &'static str) {
            tokio::time::sleep(Duration::from_millis(300)).await;
            (StatusCode::OK, "1|OK")
        }
        let app = Router::new().route("/notify", post(slow));
        let base = spawn(app).await;
        let result = deliver(
            &http_client(),
            &base,
            Duration::from_millis(50),
            &due("/notify"),
        )
        .await;
        assert!(!result.delivered());
        assert_eq!(result.status, None);
        assert_eq!(result.body, None);
    }

    #[tokio::test]
    async fn no_listener_at_all_is_a_failed_delivery() {
        let http = http_client();
        // Nothing is listening on this port — a connection error, the same
        // "not answering" bucket a timeout falls into.
        let result = deliver(
            &http,
            "http://127.0.0.1:1",
            Duration::from_millis(200),
            &due("/notify"),
        )
        .await;
        assert!(!result.delivered());
        assert_eq!(result.status, None);
    }
}
