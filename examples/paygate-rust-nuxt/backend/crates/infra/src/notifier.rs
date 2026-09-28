//! The notifier: claim one due `notifications` row with
//! `FOR UPDATE SKIP LOCKED`, deliver it, and record exactly what happened —
//! `spec.md`, "Telling the merchant".
//!
//! Three pure rules, each its own function so the unit tests `spec.md`,
//! "Layers" asks for ("claim, backoff schedule, `1|OK` matching, attempt
//! cap") exercise them without a database or an HTTP server:
//!
//! - [`is_exact_ack`] — the body must be exactly `1|OK`. Not a prefix: ECPay
//!   writes it with `echo '1|OK'; exit;`, and `/notify-loose` answers
//!   `1|OK\n` on purpose to prove the comparison is exact.
//! - [`decide_next`] — the backoff schedule and the attempt cap together, as
//!   one state transition: deliver, retry, or exhaust.
//! - the row lock itself, held for the whole HTTP call (like
//!   [`crate::relay::run_once`] and the reconciler), is what makes two
//!   notifiers deliver a due row once, not twice
//!   (`spec.md`, "Scaling": "the notifier's per-row claim").

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use paygate_domain::MerchantId;
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use crate::db::Db;
use crate::error::Result;

/// The exact body ECPay's own plugin (and every provider modelled on it)
/// requires: `1|OK`, character for character. `body.trim()` or
/// `body.starts_with("1|OK")` would both accept `/notify-loose`'s
/// `"1|OK\n"`, which `notify.feature` requires to fail.
pub fn is_exact_ack(body: &str) -> bool {
    body == "1|OK"
}

/// One claimed row.
#[derive(Debug, Clone)]
pub struct DueNotification {
    pub id: i64,
    pub payment_id: Uuid,
    pub merchant_id: MerchantId,
    pub url: String,
    pub payload: Value,
    pub attempts: i32,
}

/// What delivering one notification came back with. `status: None` is a
/// total failure to answer at all (a timeout, or a connection error) — the
/// same failed-delivery bucket as a wrong status or a wrong body, per
/// `spec.md`: "Not answering is failing."
#[derive(Debug, Clone)]
pub struct DeliveryResult {
    pub status: Option<u16>,
    pub body: Option<String>,
}

impl DeliveryResult {
    pub fn delivered(&self) -> bool {
        matches!((self.status, &self.body), (Some(200), Some(b)) if is_exact_ack(b))
    }
}

/// The state transition after one delivery attempt: deliver, retry with
/// backoff, or exhaust. `current_attempts` is the row's `attempts` BEFORE
/// this one; the returned `attempts` already counts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptOutcome {
    pub attempts: i32,
    pub delivered: bool,
    pub exhausted: bool,
    /// Meaningless once `exhausted` or `delivered`, but always computed so a
    /// caller never needs a third branch.
    pub next_delay_ms: u64,
}

/// The Nth retry (1-indexed, `attempts` after incrementing) uses
/// `backoff_ms[(N-1).min(len-1)]` (`config::NotifyConfig`'s own doc comment).
/// A cap of `max_attempts` reached exhausts the row instead of scheduling
/// another delay, whatever the schedule would have said.
pub fn decide_next(
    current_attempts: i32,
    max_attempts: u32,
    backoff_ms: &[u64],
    delivered: bool,
) -> AttemptOutcome {
    let attempts = current_attempts + 1;
    if delivered {
        return AttemptOutcome {
            attempts,
            delivered: true,
            exhausted: false,
            next_delay_ms: 0,
        };
    }
    if attempts as u32 >= max_attempts {
        return AttemptOutcome {
            attempts,
            delivered: false,
            exhausted: true,
            next_delay_ms: 0,
        };
    }
    let idx = ((attempts - 1).max(0) as usize).min(backoff_ms.len().saturating_sub(1));
    let delay = backoff_ms.get(idx).copied().unwrap_or(0);
    AttemptOutcome {
        attempts,
        delivered: false,
        exhausted: false,
        next_delay_ms: delay,
    }
}

/// One notifier pass: claim the single oldest due row (`next_attempt_at ASC,
/// id ASC`, so two outcomes written moments apart for the same payment leave
/// in the order they were written — `notify.feature`, "still owes both, in
/// order"), deliver it through `deliver`, and record the result. Returns
/// `Ok(None)` when nothing is due.
pub async fn run_one<F, Fut>(
    db: &Db,
    max_attempts: u32,
    backoff_ms: &[u64],
    deliver: F,
) -> Result<Option<AttemptOutcome>>
where
    F: FnOnce(&DueNotification) -> Fut,
    Fut: std::future::Future<Output = DeliveryResult>,
{
    let mut tx = db.begin().await?;

    let row = sqlx::query(
        "SELECT id, payment_id, merchant_id, url, payload, attempts \
           FROM notifications \
          WHERE delivered_at IS NULL AND exhausted_at IS NULL AND next_attempt_at <= now() \
          ORDER BY next_attempt_at ASC, id ASC \
          LIMIT 1 \
          FOR UPDATE SKIP LOCKED",
    )
    .fetch_optional(&mut *tx)
    .await?;

    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };

    let due = DueNotification {
        id: row.try_get("id")?,
        payment_id: row.try_get("payment_id")?,
        merchant_id: row.try_get("merchant_id")?,
        url: row.try_get("url")?,
        payload: row.try_get("payload")?,
        attempts: row.try_get("attempts")?,
    };

    let result = deliver(&due).await;
    let delivered = result.delivered();
    let outcome = decide_next(due.attempts, max_attempts, backoff_ms, delivered);

    let now = Utc::now();
    let delivered_at: Option<DateTime<Utc>> = delivered.then_some(now);
    let exhausted_at: Option<DateTime<Utc>> = outcome.exhausted.then_some(now);
    let next_attempt_at = now + ChronoDuration::milliseconds(outcome.next_delay_ms as i64);

    sqlx::query(
        "UPDATE notifications \
            SET attempts = $1, delivered_at = $2, exhausted_at = $3, next_attempt_at = $4, \
                last_status = $5, last_body = $6 \
          WHERE id = $7",
    )
    .bind(outcome.attempts)
    .bind(delivered_at)
    .bind(exhausted_at)
    .bind(next_attempt_at)
    .bind(result.status.map(|s| s as i32))
    .bind(&result.body)
    .bind(due.id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(Some(outcome))
}

/// Turn a stored notification payload back into the `application/x-www-form-urlencoded`
/// body the merchant receives — the same field vocabulary
/// `crate::repo::merchant_notify_fields` built and signed at write time,
/// already sitting in `notifications.payload` as a JSON object of strings.
pub fn payload_to_form_body(payload: &Value) -> String {
    let Some(obj) = payload.as_object() else {
        return String::new();
    };
    let mut pairs: Vec<(String, String)> = obj
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
        .collect();
    pairs.sort();
    serde_urlencoded::to_string(pairs).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_body_acknowledges() {
        assert!(is_exact_ack("1|OK"));
        assert!(!is_exact_ack("1|OK\n"));
        assert!(!is_exact_ack("0|ERROR"));
        assert!(!is_exact_ack(" 1|OK"));
        assert!(!is_exact_ack("1|OK "));
        assert!(!is_exact_ack(""));
    }

    #[test]
    fn a_200_with_the_exact_body_is_delivered() {
        let r = DeliveryResult {
            status: Some(200),
            body: Some("1|OK".to_string()),
        };
        assert!(r.delivered());
    }

    #[test]
    fn a_200_with_the_wrong_body_is_not_delivered() {
        let r = DeliveryResult {
            status: Some(200),
            body: Some("0|ERROR".to_string()),
        };
        assert!(!r.delivered());
    }

    #[test]
    fn no_answer_at_all_is_not_delivered() {
        let r = DeliveryResult {
            status: None,
            body: None,
        };
        assert!(!r.delivered());
    }

    #[test]
    fn a_redirect_status_is_not_delivered_even_with_the_right_body() {
        let r = DeliveryResult {
            status: Some(302),
            body: Some("1|OK".to_string()),
        };
        assert!(!r.delivered());
    }

    #[test]
    fn the_backoff_schedule_follows_notify_backoff_ms_in_order() {
        let schedule = [50, 100, 200, 400];
        let first = decide_next(0, 5, &schedule, false);
        assert_eq!(first.attempts, 1);
        assert_eq!(first.next_delay_ms, 50);
        let second = decide_next(1, 5, &schedule, false);
        assert_eq!(second.next_delay_ms, 100);
        let third = decide_next(2, 5, &schedule, false);
        assert_eq!(third.next_delay_ms, 200);
        let fourth = decide_next(3, 5, &schedule, false);
        assert_eq!(fourth.next_delay_ms, 400);
    }

    #[test]
    fn a_schedule_shorter_than_the_attempt_cap_repeats_its_last_entry() {
        let schedule = [50, 100];
        // max_attempts = 5, so attempt 5 would need a 5th schedule entry;
        // the 4th (last) delay is used for every attempt beyond the list.
        let d = decide_next(2, 5, &schedule, false); // attempt 3
        assert_eq!(d.next_delay_ms, 100);
    }

    #[test]
    fn reaching_max_attempts_exhausts_instead_of_scheduling_again() {
        let schedule = [50, 100, 200, 400];
        let outcome = decide_next(4, 5, &schedule, false); // this is the 5th attempt
        assert!(outcome.exhausted);
        assert!(!outcome.delivered);
        assert_eq!(outcome.attempts, 5);
    }

    #[test]
    fn a_delivered_notification_is_never_marked_exhausted() {
        let outcome = decide_next(4, 5, &[50], true);
        assert!(outcome.delivered);
        assert!(!outcome.exhausted);
    }

    #[test]
    fn nothing_ever_resends_past_the_cap() {
        // Calling decide_next again on an already-exhausted attempt count
        // (a caller bug this function refuses to make worse) still reports
        // exhausted, never a fresh schedule.
        let outcome = decide_next(5, 5, &[50], false);
        assert!(outcome.exhausted);
    }

    #[test]
    fn payload_to_form_body_url_encodes_every_field() {
        let payload = serde_json::json!({
            "MerchantTradeNo": "ACME-1",
            "RtnCode": "1",
            "CheckMacValue": "AB CD",
        });
        let body = payload_to_form_body(&payload);
        assert!(body.contains("MerchantTradeNo=ACME-1"));
        assert!(body.contains("CheckMacValue=AB%20CD") || body.contains("CheckMacValue=AB+CD"));
    }
}
