//! The one call this crate makes to a provider and waits for (`spec.md`,
//! "Refunds"), shared by the merchant-initiated `POST /payments/{id}/refunds`
//! and the automatic refund a duplicate payment earns itself
//! (`spec.md`, "When the customer pays twice"). Both reserve under the
//! payment's row lock, call the provider outside it, and record the answer —
//! `paygate_infra::repo::create_refund` / `complete_refund` do the locking and
//! the accounting; this module is only the HTTP round trip and the
//! provider-adapter plumbing between them.

use paygate_domain::{Amount, PaymentStatus};
use paygate_infra::repo;
use paygate_provider::{PlatformCredentials, RefundAnswer, RefundRequest};
use uuid::Uuid;

use crate::db;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub struct RefundResult {
    pub refund_id: Uuid,
    pub succeeded: bool,
}

/// The id a merchant-initiated refund sends the provider, derived rather
/// than randomly generated: `spec.md`, "Refunds" — "paygate sends its own
/// refund id as the provider's refund number, so the provider deduplicates a
/// retry... A refund whose answer was lost is safe to send again." A retry
/// IS the same idempotency key asking for the same amount again — the
/// fingerprint behind the key already folds the path in
/// (`idempotency.feature`'s "a key spent on one order's refund cannot be
/// spent on another's"), and this folds the amount in too, since
/// `refunds.feature`'s "A provider that refuses the refund changes nothing
/// here" reuses ONE key across two DIFFERENT amounts and those two calls
/// must NOT collide. Deriving from exactly those facts, instead of a lookup,
/// is also what keeps `refunds.feature`'s four-way race
/// (different keys, identical amount) safe: each racer derives its own,
/// different id, so none of them can ever be mistaken for one another no
/// matter how their reservations interleave.
/// Hashed with `sha2` (already a dependency here, via `auth`'s own password
/// hashing) rather than through `uuid`'s own `v5` feature, which this
/// workspace does not enable — any 16 stable bytes make a valid `Uuid`
/// (`Uuid::from_bytes` does not police the version nibble), and nothing here
/// ever needs to invert it.
fn retry_refund_id(
    merchant_id: paygate_domain::MerchantId,
    payment_id: Uuid,
    idempotency_key: &str,
    amount: Amount,
) -> Uuid {
    use sha2::{Digest, Sha256};
    let name = format!("{merchant_id}:{payment_id}:{idempotency_key}:{amount}");
    let digest = Sha256::digest(name.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

/// Reserve, call the provider, and record the answer. `attempt_id`: `None`
/// resolves to whichever attempt actually settled the order (an ordinary
/// merchant-initiated refund); `Some(id)` targets a SPECIFIC attempt (the
/// automatic duplicate-payment refund, which must claw back the SECOND
/// charge, never the first — `duplicate_payment.feature`). `payment_id` and
/// `merchant_id` are always `payment.id` / `payment.merchant_id` — taken
/// from the row directly rather than as their own parameters, which is also
/// what keeps this function's own arity in bounds. `idempotency_key`: `Some`
/// for the merchant-initiated HTTP path, so its retries can be recognised
/// (above); `None` for the automatic duplicate-payment refund, which has no
/// merchant-supplied key and keeps its old, freshly-random id every time.
pub async fn execute(
    state: &AppState,
    payment: &db::PaymentRow,
    merchant: &db::MerchantRow,
    attempt_id: Option<Uuid>,
    amount: Amount,
    reason: Option<&str>,
    idempotency_key: Option<&str>,
) -> ApiResult<RefundResult> {
    let merchant_id = payment.merchant_id;
    let payment_id = payment.id;
    let refund_id = match idempotency_key {
        Some(key) => retry_refund_id(merchant_id, payment_id, key, amount),
        None => Uuid::now_v7(),
    };

    let reservation = match db::find_pending_refund_by_id(&state.db, refund_id)
        .await
        .map_err(ApiError::from)?
    {
        Some(pending) => {
            // The same request as before, retried after its provider call
            // never answered: the reservation this id already made was
            // rolled back when that attempt was recorded as failed
            // (`paygate_infra::repo::complete_refund`), so it has to be
            // reserved again — under the SAME id, which is the whole point,
            // so `repo::create_refund`'s own `INSERT` is skipped rather than
            // made to collide with the row this id already owns.
            if !db::reserve_amount_refunded(&state.db, payment_id, amount)
                .await
                .map_err(ApiError::from)?
            {
                // The fresh-request path (`repo::create_refund`, via
                // `paygate_domain::check_refund`) reports exactly how much is
                // left when a request cannot be honoured; a resumed retry
                // deserves the same accurate answer, not a blanket "not
                // refundable" — the race this guards is a second refund
                // (a different idempotency key) taking the remainder between
                // this retry's first, timed-out attempt and its resend, which
                // leaves the payment very much still `succeeded`, just with
                // less of it left to give back.
                let current = db::find_payment_by_id_unscoped(&state.db, payment_id)
                    .await
                    .map_err(ApiError::from)?
                    .ok_or(ApiError::Internal)?;
                if current.status != PaymentStatus::Succeeded {
                    return Err(ApiError::PaymentNotRefundable);
                }
                return Err(
                    match paygate_domain::check_refund(
                        current.amount,
                        current.amount_refunded,
                        amount,
                    ) {
                        Err(paygate_domain::DomainError::RefundExceedsRemaining {
                            remaining,
                            ..
                        }) => ApiError::RefundExceedsRemaining { remaining },
                        _ => ApiError::PaymentNotRefundable,
                    },
                );
            }
            let info = db::find_attempt_provider_info(&state.db, pending.attempt_id)
                .await
                .map_err(ApiError::from)?
                .ok_or(ApiError::Internal)?;
            repo::RefundReservation {
                refund_id,
                attempt_id: pending.attempt_id,
                provider_code: info.provider_code,
                provider_trade_no: info.provider_trade_no,
                provider_charge_id: info.provider_charge_id,
                amount,
            }
        }
        None => repo::create_refund(
            &state.db,
            merchant_id,
            payment_id,
            refund_id,
            amount,
            reason,
            attempt_id,
        )
        .await
        .map_err(ApiError::from)?,
    };

    let provider_code =
        db::parse_provider_code_str(&reservation.provider_code).map_err(|_| ApiError::Internal)?;
    let provider_row = db::fetch_provider(&state.db, provider_code)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::Internal)?;

    let creds = PlatformCredentials {
        platform_id: provider_row.platform_id.clone(),
        hash_key: provider_row.hash_key.clone(),
        hash_iv: provider_row.hash_iv.clone(),
    };
    let refund_req = RefundRequest {
        refund_url: provider_row.refund_url.clone(),
        provider_merchant_id: merchant.provider_merchant_id.clone(),
        provider_trade_no: reservation.provider_trade_no.clone(),
        provider_charge_id: reservation.provider_charge_id.clone().unwrap_or_default(),
        amount,
        refund_id: refund_id.to_string(),
    };
    let adapter = paygate_provider::adapter(provider_code);
    let signed = adapter.build_refund(&creds, &refund_req);
    let url = format!("{}{}", state.provider_base_url, signed.url_path);

    let attempt = tokio::time::timeout(
        state.config.psp_timeout,
        state.http.post(&url).form(&signed.form).send(),
    )
    .await;

    let outcome = match attempt {
        Err(_) => repo::RefundOutcome::Failed { timed_out: true },
        Ok(Err(err)) => {
            tracing::warn!(error = %err, "refund call to the provider failed");
            repo::RefundOutcome::Failed { timed_out: false }
        }
        Ok(Ok(response)) => {
            if !response.status().is_success() {
                repo::RefundOutcome::Failed { timed_out: false }
            } else {
                let body = response.text().await.unwrap_or_default();
                match adapter.parse_refund_answer(&creds, &body) {
                    // paygate's own refund id IS the provider's own
                    // deduplication number (spec.md, "Refunds") — nothing
                    // else to parse out of a bare "it worked" answer.
                    Ok(RefundAnswer::Succeeded) => repo::RefundOutcome::Succeeded {
                        provider_refund_id: refund_id.to_string(),
                    },
                    _ => repo::RefundOutcome::Failed { timed_out: false },
                }
            }
        }
    };
    let succeeded = matches!(outcome, repo::RefundOutcome::Succeeded { .. });

    let merchant_ctx = repo::MerchantContext {
        merchant_id,
        provider_merchant_id: merchant.provider_merchant_id.clone(),
        merchant_trade_no: payment.merchant_trade_no.clone(),
        notify_url: payment.notify_url.clone(),
    };
    repo::complete_refund(
        &state.db,
        payment_id,
        refund_id,
        amount,
        outcome,
        &merchant_ctx,
        &merchant.hash_key,
        &merchant.hash_iv,
    )
    .await
    .map_err(ApiError::from)?;

    Ok(RefundResult {
        refund_id,
        succeeded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_key_and_amount_derive_the_same_retry_id() {
        let payment_id = Uuid::now_v7();
        let a = retry_refund_id(1, payment_id, "refund-lost-answer", 408);
        let b = retry_refund_id(1, payment_id, "refund-lost-answer", 408);
        assert_eq!(a, b, "a retry of the same request must resend the same id");
    }

    #[test]
    fn the_same_key_with_a_different_amount_derives_a_different_id() {
        // `refunds.feature`, "A provider that refuses the refund changes
        // nothing here": one key, first 119 then 1000 — two different
        // requests, so two different ids, or the second call would wrongly
        // resume the first's (already-failed, unrelated) reservation.
        let payment_id = Uuid::now_v7();
        let first = retry_refund_id(1, payment_id, "refund-provider-down", 119);
        let second = retry_refund_id(1, payment_id, "refund-provider-down", 1000);
        assert_ne!(first, second);
    }

    #[test]
    fn different_keys_for_the_same_amount_derive_different_ids() {
        // `refunds.feature`'s four-way race: four different keys, all
        // amount 4000 — each must get its own id, or two genuinely distinct,
        // concurrent refunds could be mistaken for a retry of one another.
        let payment_id = Uuid::now_v7();
        let a = retry_refund_id(1, payment_id, "race-refund-1-a", 4000);
        let b = retry_refund_id(1, payment_id, "race-refund-1-b", 4000);
        let c = retry_refund_id(1, payment_id, "race-refund-1-c", 4000);
        let d = retry_refund_id(1, payment_id, "race-refund-1-d", 4000);
        let ids = [a, b, c, d];
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(ids[i], ids[j], "racers {i} and {j} must not collide");
            }
        }
    }

    #[test]
    fn the_same_key_and_amount_on_a_different_payment_derives_a_different_id() {
        let a = retry_refund_id(1, Uuid::now_v7(), "shared-key", 1000);
        let b = retry_refund_id(1, Uuid::now_v7(), "shared-key", 1000);
        assert_ne!(a, b);
    }

    #[test]
    fn the_same_key_and_amount_for_a_different_merchant_derives_a_different_id() {
        // `idempotency.feature`, "The same key sent by two merchants is two
        // keys": the derivation has to agree.
        let payment_id = Uuid::now_v7();
        let a = retry_refund_id(1, payment_id, "shared-key", 1000);
        let b = retry_refund_id(2, payment_id, "shared-key", 1000);
        assert_ne!(a, b);
    }
}
