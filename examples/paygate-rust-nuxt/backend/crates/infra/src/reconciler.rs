//! The reconciler: the selection query (only `redirected`, only older than
//! `RECONCILE_AFTER_MINUTES`, only if `queried_at` is null or older than
//! `RECONCILE_RETRY_MINUTES`, batch-limited, `SKIP LOCKED` — see
//! [`select_candidates`]'s own doc comment for exactly which row the
//! `SKIP LOCKED` claim is taken on, and why), asking the provider with
//! `QueryTradeInfo`, verifying the answer, and driving
//! [`crate::repo::reconcile`]'s writes — `spec.md`, "Reconciling what never
//! came back". Its second job, [`resend_pending_refunds`], is documented on
//! that function.
//!
//! [`run_pass`] is the whole first job, called by `worker reconcile`'s
//! `POST /run` (`crates/worker/src/admin.rs`): one pass to completion,
//! answering how many attempts were asked about, settled, and abandoned.

use std::time::Duration;

use paygate_domain::{MerchantId, ProviderCode};
use paygate_provider::{
    adapter, PlatformCredentials, QueryAnswer, QueryRequest, RefundAnswer, RefundRequest,
};
use sqlx::Row;
use uuid::Uuid;

use crate::db::Db;
use crate::error::Result;
use crate::repo::{self, MerchantContext};

fn parse_provider_code(s: &str) -> Option<ProviderCode> {
    match s {
        "ecpay" => Some(ProviderCode::Ecpay),
        "newebpay" => Some(ProviderCode::Newebpay),
        _ => None,
    }
}

struct Candidate {
    attempt_id: Uuid,
    provider_code: ProviderCode,
    provider_trade_no: String,
    query_url: String,
    platform_creds: PlatformCredentials,
    merchant: MerchantContext,
    merchant_hash_key: String,
    merchant_hash_iv: String,
    /// `merchants.duplicate_auto_refund` — fetched here, in the same join as
    /// everything else this candidate needs, rather than assumed `true`: a
    /// duplicate the query finds is only refunded automatically when this
    /// merchant asked for that (`spec.md`, "When the customer pays twice"),
    /// the same preference the callback path reads before calling
    /// `refund_flow::execute` (`crates/api/src/routes/webhooks.rs`).
    duplicate_auto_refund: bool,
}

/// The `SKIP LOCKED` claim is taken on `payments` (`FOR UPDATE OF p`), never
/// on `payment_attempts` — deliberately the reverse of what this query used
/// to lock. `run_pass` holds this transaction open across an entire round
/// trip to the provider per candidate (`RECONCILE_HTTP_TIMEOUT_MS`, up to a
/// few seconds), and only later calls `repo::reconcile::settle_from_query`/
/// `abandon_attempt`, which lock `payments` — so whichever row THIS query
/// locks first fixes this whole transaction's lock-acquisition order for the
/// rest of the pass, no matter what those two functions do internally.
/// Locking `payment_attempts` here (the original shape) made that order
/// attempt-then-payment, the exact reverse of `repo::refunds::create_refund`
/// (payment-then-attempt) — an ABBA deadlock the instant an ordinary
/// merchant refund or callback landed on the very payment a reconciliation
/// pass was mid-provider-call on. `payments` first here, matching everyone
/// else, is what makes the transaction's order payment-then-attempt too, the
/// canonical order documented in `repo/mod.rs`.
///
/// `SKIP LOCKED` on `payments` is a STRICTLY STRONGER exclusion than on
/// `payment_attempts` ever was: two replicas can no longer even claim two
/// DIFFERENT attempts of the same payment at once, and a candidate whose
/// payment is mid-callback is skipped this pass rather than claimed —
/// `scaling.feature`'s "one query reaches the provider, not two" still
/// holds (a strictly narrower race is still no race), and skipping instead
/// of blocking is exactly what a `SKIP LOCKED` claim is for.
///
/// The `payment_attempts` rows this batch actually claimed are locked by a
/// SECOND statement, right after — plain `FOR UPDATE`, no `SKIP LOCKED`,
/// because by then this transaction already holds every one of their
/// payments' own locks: nobody else can be holding one of these exact
/// attempt rows without having taken its payment's lock first (the same
/// canonical order), which this transaction already has, so this second
/// lock can never actually block. Two statements, not one `FOR UPDATE OF p,
/// a`, because which table's rows a single multi-table `FOR UPDATE` locks
/// first is up to the planner, not something a join's own shape lets us
/// pin down — two separate statements in the same transaction is the only
/// way to guarantee the order.
async fn select_candidates(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    after: Duration,
    retry: Duration,
    batch_size: i64,
) -> Result<Vec<Candidate>> {
    let rows = sqlx::query(
        "SELECT a.id AS attempt_id, a.provider_code, a.provider_trade_no, \
                p.merchant_trade_no, p.merchant_id, p.notify_url, \
                m.provider_merchant_id, m.hash_key AS merchant_hash_key, m.hash_iv AS merchant_hash_iv, \
                m.duplicate_auto_refund, \
                pr.platform_id, pr.hash_key AS platform_hash_key, pr.hash_iv AS platform_hash_iv, \
                pr.query_url \
           FROM payment_attempts a \
           JOIN payments p ON p.id = a.payment_id \
           JOIN merchants m ON m.id = p.merchant_id \
           JOIN providers pr ON pr.code = a.provider_code \
          WHERE a.status = 'redirected' \
            AND a.started_at < now() - make_interval(secs => $1) \
            AND (a.queried_at IS NULL OR a.queried_at < now() - make_interval(secs => $2)) \
          ORDER BY a.started_at ASC \
          LIMIT $3 \
          FOR UPDATE OF p SKIP LOCKED",
    )
    .bind(after.as_secs_f64())
    .bind(retry.as_secs_f64())
    .bind(batch_size)
    .fetch_all(&mut **tx)
    .await?;

    let mut candidates = Vec::with_capacity(rows.len());
    for row in rows {
        let provider_code_raw: String = row.try_get("provider_code")?;
        let Some(provider_code) = parse_provider_code(&provider_code_raw) else {
            continue;
        };
        candidates.push(Candidate {
            attempt_id: row.try_get("attempt_id")?,
            provider_code,
            provider_trade_no: row.try_get("provider_trade_no")?,
            query_url: row.try_get("query_url")?,
            platform_creds: PlatformCredentials {
                platform_id: row.try_get("platform_id")?,
                hash_key: row.try_get("platform_hash_key")?,
                hash_iv: row.try_get("platform_hash_iv")?,
            },
            merchant: MerchantContext {
                merchant_id: row.try_get::<MerchantId, _>("merchant_id")?,
                provider_merchant_id: row.try_get("provider_merchant_id")?,
                merchant_trade_no: row.try_get("merchant_trade_no")?,
                notify_url: row.try_get("notify_url")?,
            },
            merchant_hash_key: row.try_get("merchant_hash_key")?,
            merchant_hash_iv: row.try_get("merchant_hash_iv")?,
            duplicate_auto_refund: row.try_get("duplicate_auto_refund")?,
        });
    }

    // Lock the claimed attempts themselves, second — see the doc comment
    // above for why this has to be its own statement, and why it can never
    // block now that `payments` is already locked first.
    if !candidates.is_empty() {
        let attempt_ids: Vec<Uuid> = candidates.iter().map(|c| c.attempt_id).collect();
        sqlx::query("SELECT id FROM payment_attempts WHERE id = ANY($1) FOR UPDATE")
            .bind(&attempt_ids)
            .fetch_all(&mut **tx)
            .await?;
    }

    Ok(candidates)
}

/// The selection query alone, exposed by attempt id only — an additive,
/// test-facing view onto [`select_candidates`] (`spec.md`, "Layers" asks
/// for the selection query's behaviour to be covered directly: only
/// `redirected`, only older than `after`, only when `queried_at` is null or
/// older than `retry`, batch-limited, `SKIP LOCKED`). Not used by
/// `run_pass` itself, which needs the full join; kept here rather than
/// duplicating the query so the two can never drift apart.
pub async fn select_candidate_attempt_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    after: Duration,
    retry: Duration,
    batch_size: i64,
) -> Result<Vec<Uuid>> {
    let candidates = select_candidates(tx, after, retry, batch_size).await?;
    Ok(candidates.into_iter().map(|c| c.attempt_id).collect())
}

/// What one reconcile pass did — the shape `worker reconcile`'s
/// `POST /run` answers (`crates/worker/src/admin.rs`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PassSummary {
    pub queried: u32,
    pub settled: u32,
    pub abandoned: u32,
    pub refunds_resent: u32,
    /// A duplicate charge the query itself found (`ReconcileOutcome::Duplicate`)
    /// and, per `merchants.duplicate_auto_refund`, queued a `pending` refund
    /// for — dropped on the floor for every outcome that was not `Settled`
    /// before this field existed, `spec.md`'s "the money is refunded
    /// automatically" was silently untrue for exactly this path.
    /// `refunds_resent` (above) then delivers it, this pass or the next.
    pub duplicate_refunds_queued: u32,
}

/// One full reconcile pass: select every eligible attempt, ask the provider
/// about each in turn, and write whatever the (verified) answer means — all
/// in ONE transaction, so the `SKIP LOCKED` claim from [`select_candidates`]
/// (on `payments`, not `payment_attempts` — see its own doc comment) is held
/// across every one of these `await`s and a second replica running the same
/// pass concurrently can never even claim a DIFFERENT attempt of a payment
/// this one already claimed (`scaling.feature`, "one query reaches the
/// provider, not two" — a strictly narrower race than "the same attempt"
/// alone is still no race).
pub async fn run_pass(
    db: &Db,
    http: &reqwest::Client,
    provider_base_url: &str,
    after: Duration,
    retry: Duration,
    batch_size: i64,
    http_timeout: Duration,
) -> Result<PassSummary> {
    let mut tx = db.begin().await?;
    let candidates = select_candidates(&mut tx, after, retry, batch_size).await?;

    let mut summary = PassSummary::default();
    for candidate in candidates {
        summary.queried += 1;
        let a = adapter(candidate.provider_code);
        let query = QueryRequest {
            query_url: candidate.query_url.clone(),
            provider_merchant_id: candidate.merchant.provider_merchant_id.clone(),
            provider_trade_no: candidate.provider_trade_no.clone(),
        };
        let signed = a.build_query(&candidate.platform_creds, &query);
        let url = format!("{provider_base_url}{}", signed.url_path);

        let http_result = http
            .post(&url)
            .timeout(http_timeout)
            .form(&signed.form)
            .send()
            .await;

        let body = match http_result {
            Ok(resp) if resp.status().is_success() => resp.text().await.unwrap_or_default(),
            Ok(resp) => {
                // A non-2xx (the mock's `403` throttle among them) still
                // counts as an asking, or the next pass would queue up
                // another one and extend the blackout.
                let status = resp.status();
                repo::record_unresolved_query(
                    &mut tx,
                    candidate.attempt_id,
                    None,
                    &format!("http {status}"),
                )
                .await?;
                continue;
            }
            Err(e) => {
                repo::record_unresolved_query(
                    &mut tx,
                    candidate.attempt_id,
                    None,
                    &format!("request failed: {e}"),
                )
                .await?;
                continue;
            }
        };

        let answer = a.parse_query_answer(&candidate.platform_creds, &body);
        match answer {
            Err(_) => {
                // A forged or malformed answer is worth exactly what no
                // answer is worth.
                repo::record_unresolved_query(&mut tx, candidate.attempt_id, None, &body).await?;
            }
            Ok(QueryAnswer::Unpaid) => {
                repo::record_unresolved_query(&mut tx, candidate.attempt_id, Some("0"), &body)
                    .await?;
            }
            Ok(QueryAnswer::NeverCompleted) => {
                repo::abandon_attempt(&mut tx, candidate.attempt_id, &body).await?;
                summary.abandoned += 1;
            }
            Ok(QueryAnswer::Paid { facts }) => {
                let application = repo::settle_from_query(
                    &mut tx,
                    candidate.attempt_id,
                    &facts,
                    &body,
                    &candidate.merchant,
                    &candidate.merchant_hash_key,
                    &candidate.merchant_hash_iv,
                )
                .await?;
                match application.outcome {
                    repo::ReconcileOutcome::Settled => summary.settled += 1,
                    repo::ReconcileOutcome::Duplicate => {
                        // `settle_from_query` recorded `PaymentDuplicatePaid`
                        // already; giving it back is this function's own
                        // job, the same division of labour the callback path
                        // uses (`crates/api/src/routes/webhooks.rs`, ~line
                        // 144). `crates/infra` cannot call that crate's
                        // `refund_flow`, and does not need to: creating the
                        // refund row `pending`, in the SAME transaction that
                        // is still holding this attempt's and payment's row
                        // locks, and leaving it for `resend_pending_refunds`
                        // to deliver, is the smallest correct change
                        // (`spec.md`, "Reconciling what never came back":
                        // "both are safe to repeat").
                        if candidate.duplicate_auto_refund {
                            if let Some(amount) = application.duplicate_amount {
                                repo::create_duplicate_refund(
                                    &mut tx,
                                    candidate.merchant.merchant_id,
                                    application.payment_id,
                                    application.attempt_id,
                                    amount,
                                )
                                .await?;
                                summary.duplicate_refunds_queued += 1;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    tx.commit().await?;
    Ok(summary)
}

struct PendingRefund {
    refund_id: Uuid,
    payment_id: Uuid,
    amount: paygate_domain::Amount,
    provider_code: ProviderCode,
    provider_trade_no: String,
    provider_charge_id: Option<String>,
    refund_url: String,
    platform_creds: PlatformCredentials,
    merchant: MerchantContext,
    merchant_hash_key: String,
    merchant_hash_iv: String,
}

async fn select_pending_refunds(db: &Db, batch_size: i64) -> Result<Vec<PendingRefund>> {
    // A lighter claim than `select_candidates`'s: re-sending a `pending`
    // refund is declared SAFE to repeat, because the provider deduplicates
    // on the refund id paygate already chose (`spec.md`, "Reconciling what
    // never came back": "both are safe to repeat"). `SKIP LOCKED` still
    // avoids the common case of two reconciler passes racing the same scan;
    // it does not need to survive the outbound call the way the query job's
    // must, so this transaction commits (releasing the row lock) before any
    // network call is made.
    let mut tx = db.begin().await?;
    let rows = sqlx::query(
        "SELECT r.id AS refund_id, r.payment_id, r.amount, a.provider_code, a.provider_trade_no, \
                a.provider_charge_id, p.merchant_trade_no, p.merchant_id, p.notify_url, \
                m.provider_merchant_id, m.hash_key AS merchant_hash_key, m.hash_iv AS merchant_hash_iv, \
                pr.platform_id, pr.hash_key AS platform_hash_key, pr.hash_iv AS platform_hash_iv, \
                pr.refund_url \
           FROM refunds r \
           JOIN payment_attempts a ON a.id = r.attempt_id \
           JOIN payments p ON p.id = r.payment_id \
           JOIN merchants m ON m.id = p.merchant_id \
           JOIN providers pr ON pr.code = a.provider_code \
          WHERE r.status = 'pending' \
          ORDER BY r.created_at ASC \
          LIMIT $1 \
          FOR UPDATE OF r SKIP LOCKED",
    )
    .bind(batch_size)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let provider_code_raw: String = row.try_get("provider_code")?;
        let Some(provider_code) = parse_provider_code(&provider_code_raw) else {
            continue;
        };
        out.push(PendingRefund {
            refund_id: row.try_get("refund_id")?,
            payment_id: row.try_get("payment_id")?,
            amount: row.try_get("amount")?,
            provider_code,
            provider_trade_no: row.try_get("provider_trade_no")?,
            provider_charge_id: row.try_get("provider_charge_id")?,
            refund_url: row.try_get("refund_url")?,
            platform_creds: PlatformCredentials {
                platform_id: row.try_get("platform_id")?,
                hash_key: row.try_get("platform_hash_key")?,
                hash_iv: row.try_get("platform_hash_iv")?,
            },
            merchant: MerchantContext {
                merchant_id: row.try_get::<MerchantId, _>("merchant_id")?,
                provider_merchant_id: row.try_get("provider_merchant_id")?,
                merchant_trade_no: row.try_get("merchant_trade_no")?,
                notify_url: row.try_get("notify_url")?,
            },
            merchant_hash_key: row.try_get("merchant_hash_key")?,
            merchant_hash_iv: row.try_get("merchant_hash_iv")?,
        });
    }
    Ok(out)
}

/// The reconciler's second job: re-send every refund still `pending` —
/// including a duplicate payment's own failed automatic refund
/// (`duplicate_payment.feature`, "a duplicate refund the provider refused is
/// sent again, and lands once").
pub async fn resend_pending_refunds(
    db: &Db,
    http: &reqwest::Client,
    provider_base_url: &str,
    batch_size: i64,
    http_timeout: Duration,
) -> Result<u32> {
    let pending = select_pending_refunds(db, batch_size).await?;
    let mut resent = 0u32;

    for refund in pending {
        let a = adapter(refund.provider_code);
        let req = RefundRequest {
            refund_url: refund.refund_url.clone(),
            provider_merchant_id: refund.merchant.provider_merchant_id.clone(),
            provider_trade_no: refund.provider_trade_no.clone(),
            provider_charge_id: refund.provider_charge_id.clone().unwrap_or_default(),
            amount: refund.amount,
            refund_id: refund.refund_id.to_string(),
        };
        let signed = a.build_refund(&refund.platform_creds, &req);
        let url = format!("{provider_base_url}{}", signed.url_path);

        let http_result = http
            .post(&url)
            .timeout(http_timeout)
            .form(&signed.form)
            .send()
            .await;

        let outcome = match http_result {
            Ok(resp) if resp.status().is_success() => {
                let body = resp.text().await.unwrap_or_default();
                match a.parse_refund_answer(&refund.platform_creds, &body) {
                    Ok(RefundAnswer::Succeeded) => {
                        // The mock does not hand back a distinct provider
                        // refund id beyond what paygate itself sent; the
                        // refund id paygate chose IS the number the provider
                        // deduplicates on (spec.md, "Refunds").
                        Some(repo::RefundOutcome::Succeeded {
                            provider_refund_id: refund.refund_id.to_string(),
                        })
                    }
                    _ => None,
                }
            }
            _ => None,
        };

        if let Some(outcome) = outcome {
            repo::complete_refund(
                db,
                refund.payment_id,
                refund.refund_id,
                refund.amount,
                outcome,
                &refund.merchant,
                &refund.merchant_hash_key,
                &refund.merchant_hash_iv,
            )
            .await?;
            resent += 1;
        }
        // A refund that is still not answered stays `pending`; nothing to
        // update — the next pass tries again, safely, forever if it must.
    }
    Ok(resent)
}
