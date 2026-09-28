//! Session, API keys and reports for a merchant's staff
//! (`openapi.yaml`, tag `Dashboard`).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::NaiveDate;
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{sha256_hex, DashboardSession};
use crate::cookies;
use crate::db;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginBody {
    email: Value,
    password: Value,
}

fn as_str_field(v: &Value) -> Option<&str> {
    v.as_str()
}

/// A hash generated once, at the same cost (10) the fixtures themselves use,
/// purely so an unknown email still costs one real bcrypt verification —
/// never a value this module invented ad hoc, since a malformed hash would
/// let `bcrypt::verify` return early without running the expensive key
/// schedule at all, which is exactly the shortcut this constant exists to
/// avoid. Generated with `bcrypt::hash("paygate-dummy-timing-parity", 10)`;
/// see this module's own tests for the check that keeps it honest.
const DUMMY_HASH: &str = "$2b$10$.eY0syvnP0dCpAIEtYA48O1NYT9uAjhaalb80witQTowd9dMwo4vS";

/// Whether `password` matches `stored_hash` — or, when there is no such
/// user, whether it matches a fixed dummy hash instead. Either way exactly
/// one `bcrypt::verify` call runs, so an unknown email and a wrong password
/// answer in the same time and the same code
/// (`dashboard_sessions.feature`: "one answer, whichever half was wrong").
fn verify_password(password: &str, stored_hash: Option<&str>) -> bool {
    match stored_hash {
        Some(hash) => bcrypt::verify(password, hash).unwrap_or(false),
        None => {
            let _ = bcrypt::verify(password, DUMMY_HASH);
            false
        }
    }
}

pub async fn login(State(state): State<AppState>, body: axum::body::Bytes) -> ApiResult<Response> {
    let raw: Value = crate::dto::parse_json_body(&body)?;
    let parsed: LoginBody = serde_json::from_value(raw).map_err(|_| ApiError::InvalidRequest)?;
    let email = as_str_field(&parsed.email).ok_or(ApiError::InvalidRequest)?;
    let password = as_str_field(&parsed.password).ok_or(ApiError::InvalidRequest)?;

    let user = db::find_dashboard_user_by_email(&state.db, email)
        .await
        .map_err(ApiError::from)?;

    let ok = verify_password(password, user.as_ref().map(|u| u.password_hash.as_str()));
    let user = match (ok, user) {
        (true, Some(u)) => u,
        _ => return Err(ApiError::InvalidCredentials),
    };

    let merchant = db::fetch_merchant(&state.db, user.merchant_id)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::InvalidCredentials)?;

    // A random token, freshly minted regardless of any `session` cookie the
    // caller showed up with (`dashboard_sessions.feature`: "Logging in never
    // adopts a session id the client already had").
    let mut raw_token = [0u8; 32];
    use rand::RngCore;
    rand::rngs::OsRng.fill_bytes(&mut raw_token);
    let token = hex::encode(raw_token);
    let token_hash = sha256_hex(&token);

    let payload = json!({ "merchant_id": merchant.id, "email": user.email });
    if crate::state::redis_bounded(state.redis.create_session(
        &token_hash,
        &payload,
        state.config.session_ttl,
    ))
    .await
    .is_err()
    {
        return Err(ApiError::SessionStoreUnavailable);
    }

    let secure = state.config.cookie_secure;
    let ttl_secs = state.config.session_ttl.as_secs();
    let body = json!({
        "email": user.email,
        "merchant": {
            "id": merchant.id,
            "name": merchant.name,
            "currency": merchant.currency,
            "timezone": merchant.timezone,
        }
    });
    let mut response = (StatusCode::OK, Json(body)).into_response();
    let headers = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&cookies::session_cookie(&token, ttl_secs, secure)) {
        headers.append("Set-Cookie", v);
    }
    if let Ok(v) = HeaderValue::from_str(&cookies::session_hint_cookie(ttl_secs, secure)) {
        headers.append("Set-Cookie", v);
    }
    Ok(response)
}

pub async fn logout(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> ApiResult<Response> {
    if let Some(cookie_header) = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
    {
        for part in cookie_header.split(';') {
            let part = part.trim();
            if let Some(token) = part.strip_prefix("session=") {
                if !token.is_empty() {
                    let _ =
                        crate::state::redis_bounded(state.redis.delete_session(&sha256_hex(token)))
                            .await;
                }
            }
        }
    }
    let secure = state.config.cookie_secure;
    let mut response = StatusCode::NO_CONTENT.into_response();
    let out = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&cookies::clear_session_cookie(secure)) {
        out.append("Set-Cookie", v);
    }
    if let Ok(v) = HeaderValue::from_str(&cookies::clear_session_hint_cookie(secure)) {
        out.append("Set-Cookie", v);
    }
    Ok(response)
}

pub async fn me(
    State(state): State<AppState>,
    session: DashboardSession,
) -> ApiResult<Json<Value>> {
    let merchant = db::fetch_merchant(&state.db, session.merchant_id)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::Unauthenticated)?;
    Ok(Json(json!({
        "email": session.email,
        "merchant": {
            "id": merchant.id,
            "name": merchant.name,
            "currency": merchant.currency,
            "timezone": merchant.timezone,
        }
    })))
}

fn merchant_slug(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(4)
        .collect();
    if slug.is_empty() {
        "shop".to_string()
    } else {
        slug
    }
}

pub async fn create_api_key(
    State(state): State<AppState>,
    session: DashboardSession,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let merchant = db::fetch_merchant(&state.db, session.merchant_id)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::Unauthenticated)?;

    let key_prefix = format!("sk_test_{}", merchant_slug(&merchant.name));
    let suffix: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();
    let raw_key = format!("{key_prefix}_{suffix}");
    let key_hash = sha256_hex(&raw_key);

    let created = db::insert_api_key(&state.db, merchant.id, &key_hash, &key_prefix)
        .await
        .map_err(ApiError::from)?;

    // `201`, not `200`: a key is a new resource, and `createApiKey` in
    // spec/openapi/openapi.yaml documents exactly one success status.
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": created.id,
            "key": raw_key,
            "key_prefix": key_prefix,
            "created_at": created.created_at.to_rfc3339(),
        })),
    ))
}

pub async fn revoke_api_key(
    State(state): State<AppState>,
    session: DashboardSession,
    Path(key_id): Path<i64>,
) -> ApiResult<StatusCode> {
    let key_hash = db::revoke_api_key(&state.db, session.merchant_id, key_id)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::ApiKeyNotFound)?;
    let _ = crate::state::redis_bounded(state.redis.invalidate_api_key(&key_hash)).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct ReportQuery {
    from: Option<String>,
    to: Option<String>,
}

const MAX_REPORT_RANGE_DAYS: i64 = 92;

fn parse_report_date(raw: &str, field: &'static str) -> ApiResult<NaiveDate> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| ApiError::ValidationFailed { field })
}

pub async fn daily_report(
    State(state): State<AppState>,
    session: DashboardSession,
    Query(query): Query<ReportQuery>,
) -> ApiResult<Json<Value>> {
    let merchant = db::fetch_merchant(&state.db, session.merchant_id)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::Unauthenticated)?;

    let tz: chrono_tz::Tz = merchant.timezone.parse().unwrap_or(chrono_tz::UTC);
    let today = chrono::Utc::now().with_timezone(&tz).date_naive();

    let to = match &query.to {
        Some(raw) => parse_report_date(raw, "to")?,
        None => today,
    };
    let from = match &query.from {
        Some(raw) => parse_report_date(raw, "from")?,
        None => to - chrono::Duration::days(6),
    };
    if from > to {
        return Err(ApiError::ValidationFailed { field: "from" });
    }
    if (to - from).num_days() + 1 > MAX_REPORT_RANGE_DAYS {
        return Err(ApiError::RangeTooLarge);
    }

    let report = state
        .clickhouse
        .daily_report(
            merchant.id,
            &merchant.currency,
            &merchant.timezone,
            from,
            to,
        )
        .await
        .map_err(ApiError::from)?;

    let days: Vec<Value> = report
        .days
        .iter()
        .map(|d| {
            json!({
                "date": d.date.to_string(),
                "succeeded_count": d.succeeded_count,
                "failed_count": d.failed_count,
                "refunded_count": d.refunded_count,
                "gross_amount": d.gross_amount,
                "refunded_amount": d.refunded_amount,
                "net_amount": d.net_amount,
                "success_rate_bps": d.success_rate_bps,
            })
        })
        .collect();

    Ok(Json(json!({
        "currency": report.currency,
        "timezone": report.timezone,
        "from": report.from.to_string(),
        "to": report.to.to_string(),
        "days": days,
        "totals": {
            "succeeded_count": report.totals.succeeded_count,
            "failed_count": report.totals.failed_count,
            "refunded_count": report.totals.refunded_count,
            "gross_amount": report.totals.gross_amount,
            "refunded_amount": report.totals.refunded_amount,
            "net_amount": report.totals.net_amount,
            "success_rate_bps": report.totals.success_rate_bps,
            "declines": report.totals.declines,
        }
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_this_repos_own_seeded_fixture() {
        // Every feature file's Background seeds this exact hash for the
        // password "secret123" (api_keys.feature, dashboard_sessions.feature,
        // resilience.feature, reports.feature) — this is the assertion that
        // proves the login path works against the data the features really
        // insert, not just against a hash this crate generated itself.
        let hash = "$2a$10$MwKxT13/lPxFMjvrkm0dL.QJuNll1zljDU.eOoRVvgrWF.kf/hyC2";
        assert!(verify_password("secret123", Some(hash)));
        assert!(!verify_password("wrongpass", Some(hash)));
        assert!(!verify_password("secret1234", Some(hash)));
    }

    #[test]
    fn an_unknown_email_still_performs_a_genuine_bcrypt_verification_not_a_short_circuit() {
        // The honest version of "an unknown email does not short-circuit":
        // a timing test would be flaky, so instead this asserts the thing
        // that actually makes the timing property true — `DUMMY_HASH` is
        // itself a valid, verifiable bcrypt hash. If it were malformed,
        // `bcrypt::verify` would return `Err` immediately without ever
        // running the expensive key schedule, and the "no such user" branch
        // would be cheaper than a real one despite `verify_password` still
        // compiling and returning `false` either way.
        assert!(bcrypt::verify("paygate-dummy-timing-parity", DUMMY_HASH).unwrap());
        assert!(!verify_password("whatever", None));
    }

    #[test]
    fn the_api_keys_feature_fixtures_slug_are_reproduced_from_the_merchant_name() {
        // api_keys.feature seeds `key_prefix = 'sk_test_acme'` for "Acme
        // Coffee" and asserts a freshly-minted key gets the same prefix.
        assert_eq!(merchant_slug("Acme Coffee"), "acme");
        // Globex's own seeded prefix, `sk_test_glob`, is consistent with the
        // same rule even though no scenario mints a fresh Globex key.
        assert_eq!(merchant_slug("Globex"), "glob");
    }

    #[test]
    fn punctuation_and_case_are_stripped_before_truncating_to_four() {
        assert_eq!(merchant_slug("D'Angelo's Diner"), "dang");
        assert_eq!(merchant_slug("A B"), "ab");
    }

    #[test]
    fn a_name_with_no_alphanumerics_falls_back_rather_than_producing_an_empty_prefix() {
        assert_eq!(merchant_slug("!!!"), "shop");
        assert_eq!(merchant_slug(""), "shop");
    }

    #[test]
    fn a_report_date_must_be_iso_8601_not_any_other_common_format() {
        assert!(parse_report_date("2026-09-18", "from").is_ok());
        assert!(matches!(
            parse_report_date("09-18-2026", "to").unwrap_err(),
            ApiError::ValidationFailed { field: "to" }
        ));
        assert!(matches!(
            parse_report_date("not-a-date", "from").unwrap_err(),
            ApiError::ValidationFailed { field: "from" }
        ));
    }
}
