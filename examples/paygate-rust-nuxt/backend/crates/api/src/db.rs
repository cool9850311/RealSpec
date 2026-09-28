//! Read paths this crate needs that `paygate-infra::repo` does not expose
//! (row mapping there is `pub(crate)`, and nothing in `repo` hands back a
//! `merchants`, `providers`, `api_keys` or `dashboard_users` row at all,
//! or looks a payment up by anything other than `id` under a lock the
//! request holds for a write). Runtime queries only, same as every other
//! crate here: no compile-time DB.

use chrono::{DateTime, Utc};
use paygate_domain::{Amount, AttemptStatus, MerchantId, PaymentStatus, ProviderCode};
use sqlx::Row;
use uuid::Uuid;

use paygate_infra::{Db, Error, Result};

#[derive(Debug, Clone)]
pub struct MerchantRow {
    pub id: MerchantId,
    pub name: String,
    pub currency: String,
    pub timezone: String,
    pub provider_code: ProviderCode,
    pub provider_merchant_id: String,
    pub duplicate_auto_refund: bool,
    pub rate_limit_per_minute: u32,
    pub hash_key: String,
    pub hash_iv: String,
}

pub fn parse_provider_code_str(s: &str) -> Result<ProviderCode> {
    parse_provider_code(s)
}

fn parse_provider_code(s: &str) -> Result<ProviderCode> {
    match s {
        "ecpay" => Ok(ProviderCode::Ecpay),
        "newebpay" => Ok(ProviderCode::Newebpay),
        other => Err(Error::Database(sqlx::Error::ColumnDecode {
            index: "provider_code".to_string(),
            source: format!("{other:?} is not a known provider_code").into(),
        })),
    }
}

pub async fn fetch_merchant(db: &Db, merchant_id: MerchantId) -> Result<Option<MerchantRow>> {
    let row = sqlx::query(
        "SELECT id, name, currency, timezone, provider_code, provider_merchant_id, \
                duplicate_auto_refund, rate_limit_per_minute, hash_key, hash_iv \
           FROM merchants WHERE id = $1",
    )
    .bind(merchant_id)
    .fetch_optional(&db.0)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let provider_code: String = row.try_get("provider_code")?;
    let rate_limit: i32 = row.try_get("rate_limit_per_minute")?;
    Ok(Some(MerchantRow {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        currency: row.try_get("currency")?,
        timezone: row.try_get("timezone")?,
        provider_code: parse_provider_code(&provider_code)?,
        provider_merchant_id: row.try_get("provider_merchant_id")?,
        duplicate_auto_refund: row.try_get("duplicate_auto_refund")?,
        rate_limit_per_minute: rate_limit.max(0) as u32,
        hash_key: row.try_get("hash_key")?,
        hash_iv: row.try_get("hash_iv")?,
    }))
}

/// This crate's own use of `providers` never reaches `query_url` (that is
/// the reconciler's column, in the `worker` binary) so it is not carried
/// here at all — a struct with a field nothing reads is exactly the kind of
/// drift a reviewer has to double-check by hand.
#[derive(Debug, Clone)]
pub struct ProviderRow {
    pub platform_id: String,
    pub hash_key: String,
    pub hash_iv: String,
    pub cashier_url: String,
    pub refund_url: String,
}

pub async fn fetch_provider(db: &Db, code: ProviderCode) -> Result<Option<ProviderRow>> {
    let code_str = match code {
        ProviderCode::Ecpay => "ecpay",
        ProviderCode::Newebpay => "newebpay",
    };
    let row = sqlx::query(
        "SELECT platform_id, hash_key, hash_iv, cashier_url, refund_url \
           FROM providers WHERE code = $1",
    )
    .bind(code_str)
    .fetch_optional(&db.0)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(ProviderRow {
        platform_id: row.try_get("platform_id")?,
        hash_key: row.try_get("hash_key")?,
        hash_iv: row.try_get("hash_iv")?,
        cashier_url: row.try_get("cashier_url")?,
        refund_url: row.try_get("refund_url")?,
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentRow {
    pub id: Uuid,
    pub merchant_id: MerchantId,
    pub merchant_trade_no: String,
    pub amount: Amount,
    pub currency: String,
    pub status: PaymentStatus,
    pub item_desc: String,
    pub card_brand: Option<String>,
    pub card_last4: Option<String>,
    pub notify_url: String,
    pub client_back_url: String,
    pub amount_refunded: Amount,
    pub created_at: DateTime<Utc>,
}

fn parse_payment_status(s: &str) -> Result<PaymentStatus> {
    match s {
        "pending" => Ok(PaymentStatus::Pending),
        "succeeded" => Ok(PaymentStatus::Succeeded),
        "refunded" => Ok(PaymentStatus::Refunded),
        other => Err(Error::Database(sqlx::Error::ColumnDecode {
            index: "status".to_string(),
            source: format!("{other:?} is not a known payment status").into(),
        })),
    }
}

fn map_payment_row(row: &sqlx::postgres::PgRow) -> Result<PaymentRow> {
    let status: String = row.try_get("status")?;
    Ok(PaymentRow {
        id: row.try_get("id")?,
        merchant_id: row.try_get("merchant_id")?,
        merchant_trade_no: row.try_get("merchant_trade_no")?,
        amount: row.try_get("amount")?,
        currency: row.try_get("currency")?,
        status: parse_payment_status(&status)?,
        item_desc: row.try_get("item_desc")?,
        card_brand: row.try_get("card_brand")?,
        card_last4: row.try_get("card_last4")?,
        notify_url: row.try_get("notify_url")?,
        client_back_url: row.try_get("client_back_url")?,
        amount_refunded: row.try_get("amount_refunded")?,
        created_at: row.try_get("created_at")?,
    })
}

const PAYMENT_COLUMNS: &str = "id, merchant_id, merchant_trade_no, amount, currency, status, \
     item_desc, card_brand, card_last4, notify_url, client_back_url, amount_refunded, created_at";

/// A merchant's own order, by paygate's id — the read behind `GET
/// /payments/{paymentId}`. Scoped to `merchant_id` so another merchant's
/// order and an unknown id are the same `404` (`orders.feature`).
pub async fn find_payment_by_id(
    db: &Db,
    payment_id: Uuid,
    merchant_id: MerchantId,
) -> Result<Option<PaymentRow>> {
    let row = sqlx::query(&format!(
        "SELECT {PAYMENT_COLUMNS} FROM payments WHERE id = $1 AND merchant_id = $2"
    ))
    .bind(payment_id)
    .bind(merchant_id)
    .fetch_optional(&db.0)
    .await?;
    row.as_ref().map(map_payment_row).transpose()
}

/// By the merchant's own order number — `GET /payments?merchant_trade_no=`
/// and the "does this order already exist" half of `POST /payments`'s
/// pinned rule (`spec.md`, "Two kinds of duplicate"; `handoff.feature`).
pub async fn find_payment_by_merchant_trade_no(
    db: &Db,
    merchant_id: MerchantId,
    merchant_trade_no: &str,
) -> Result<Option<PaymentRow>> {
    let row = sqlx::query(&format!(
        "SELECT {PAYMENT_COLUMNS} FROM payments WHERE merchant_id = $1 AND merchant_trade_no = $2"
    ))
    .bind(merchant_id)
    .bind(merchant_trade_no)
    .fetch_optional(&db.0)
    .await?;
    row.as_ref().map(map_payment_row).transpose()
}

/// By id alone, with no merchant filter — used only once a caller has
/// already established the merchant boundary another way (a provider
/// callback, matched by its own `provider_trade_no` first).
pub async fn find_payment_by_id_unscoped(db: &Db, payment_id: Uuid) -> Result<Option<PaymentRow>> {
    let row = sqlx::query(&format!(
        "SELECT {PAYMENT_COLUMNS} FROM payments WHERE id = $1"
    ))
    .bind(payment_id)
    .fetch_optional(&db.0)
    .await?;
    row.as_ref().map(map_payment_row).transpose()
}

#[derive(Debug, Clone)]
pub struct AttemptSummary {
    pub provider_code: ProviderCode,
    pub status: AttemptStatus,
    pub failure_code: Option<String>,
}

fn parse_attempt_status(s: &str) -> Result<AttemptStatus> {
    match s {
        "redirected" => Ok(AttemptStatus::Redirected),
        "succeeded" => Ok(AttemptStatus::Succeeded),
        "failed" => Ok(AttemptStatus::Failed),
        "abandoned" => Ok(AttemptStatus::Abandoned),
        other => Err(Error::Database(sqlx::Error::ColumnDecode {
            index: "status".to_string(),
            source: format!("{other:?} is not a known attempt status").into(),
        })),
    }
}

/// The most recent attempt against a payment — `Payment.last_attempt`
/// (`openapi.yaml`): "what became of the most recent form".
pub async fn find_latest_attempt(db: &Db, payment_id: Uuid) -> Result<Option<AttemptSummary>> {
    let row = sqlx::query(
        "SELECT provider_code, status, failure_code FROM payment_attempts \
          WHERE payment_id = $1 ORDER BY started_at DESC LIMIT 1",
    )
    .bind(payment_id)
    .fetch_optional(&db.0)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let provider_code: String = row.try_get("provider_code")?;
    let status: String = row.try_get("status")?;
    Ok(Some(AttemptSummary {
        provider_code: parse_provider_code(&provider_code)?,
        status: parse_attempt_status(&status)?,
        failure_code: row.try_get("failure_code")?,
    }))
}

/// A refund reservation still `pending` under this exact id — `refund_flow`'s
/// own doc comment: a retry that derives the SAME id as an earlier attempt
/// whose provider call never answered finds its reservation here and resumes
/// it, rather than colliding with `refunds.id`'s primary key by trying to
/// insert it again. Looked up by that primary key alone (never by amount),
/// which is what keeps it from ever matching a genuinely different,
/// concurrent refund of the same amount (`refunds.feature`'s own four-way
/// race) — that one derives a different id in the first place.
#[derive(Debug, Clone)]
pub struct PendingRefundRow {
    pub attempt_id: Uuid,
}

pub async fn find_pending_refund_by_id(
    db: &Db,
    refund_id: Uuid,
) -> Result<Option<PendingRefundRow>> {
    let row = sqlx::query("SELECT attempt_id FROM refunds WHERE id = $1 AND status = 'pending'")
        .bind(refund_id)
        .fetch_optional(&db.0)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(PendingRefundRow {
        attempt_id: row.try_get("attempt_id")?,
    }))
}

/// The provider-facing half of a `payment_attempts` row, by id — what
/// resuming a reservation (above) needs to rebuild the exact request its
/// first, unanswered attempt already sent.
#[derive(Debug, Clone)]
pub struct AttemptProviderInfo {
    pub provider_code: String,
    pub provider_trade_no: String,
    pub provider_charge_id: Option<String>,
}

pub async fn find_attempt_provider_info(
    db: &Db,
    attempt_id: Uuid,
) -> Result<Option<AttemptProviderInfo>> {
    let row = sqlx::query(
        "SELECT provider_code, provider_trade_no, provider_charge_id \
           FROM payment_attempts WHERE id = $1",
    )
    .bind(attempt_id)
    .fetch_optional(&db.0)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(AttemptProviderInfo {
        provider_code: row.try_get("provider_code")?,
        provider_trade_no: row.try_get("provider_trade_no")?,
        provider_charge_id: row.try_get("provider_charge_id")?,
    }))
}

/// Re-reserve, atomically, the amount a resumed reservation (above) already
/// had rolled back when its first attempt was recorded as failed
/// (`paygate_infra::repo::complete_refund`'s `Failed` branch gives the money
/// back so the total is honest while nobody can prove it moved —
/// `refunds.feature`, "A refund whose answer was lost is safe to send
/// again"). One `UPDATE` re-states `check_refund`'s own rule
/// (`0 < req <= amount - amount_refunded`) as its own `WHERE` clause instead
/// of a separate read-then-write, so nothing between deciding to resume and
/// writing it can invalidate what was decided; `payments`'s own
/// `CHECK (amount_refunded BETWEEN 0 AND amount)` is the backstop either way.
/// `Ok(false)` — nothing updated — means the reservation cannot be resumed
/// (the payment closed, or something else moved the total, in the meantime).
pub async fn reserve_amount_refunded(db: &Db, payment_id: Uuid, amount: Amount) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE payments SET amount_refunded = amount_refunded + $1, updated_at = now() \
          WHERE id = $2 AND status = 'succeeded' AND amount_refunded + $1 <= amount",
    )
    .bind(amount)
    .bind(payment_id)
    .execute(&db.0)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// The api-key row an `Authorization: Bearer` presents, looked up by the
/// SHA-256 of the raw key (`spec.md`, "Credentials in this repository":
/// `api_keys.feature` asserts that only its SHA-256 is stored). `None` for
/// an unknown OR revoked key — the same refusal either way
/// (`api_keys.feature`).
#[derive(Debug, Clone)]
pub struct ApiKeyRow {
    pub merchant_id: MerchantId,
}

pub async fn find_active_api_key(db: &Db, key_hash: &str) -> Result<Option<ApiKeyRow>> {
    let row =
        sqlx::query("SELECT merchant_id FROM api_keys WHERE key_hash = $1 AND revoked_at IS NULL")
            .bind(key_hash)
            .fetch_optional(&db.0)
            .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(ApiKeyRow {
        merchant_id: row.try_get("merchant_id")?,
    }))
}

pub struct NewApiKey {
    pub id: i64,
    pub created_at: DateTime<Utc>,
}

pub async fn insert_api_key(
    db: &Db,
    merchant_id: MerchantId,
    key_hash: &str,
    key_prefix: &str,
) -> Result<NewApiKey> {
    let row = sqlx::query(
        "INSERT INTO api_keys (merchant_id, key_hash, key_prefix) VALUES ($1, $2, $3) \
         RETURNING id, now() AS created_at",
    )
    .bind(merchant_id)
    .bind(key_hash)
    .bind(key_prefix)
    .fetch_one(&db.0)
    .await?;
    Ok(NewApiKey {
        id: row.try_get("id")?,
        created_at: row.try_get("created_at")?,
    })
}

/// `None` when the key does not exist for this merchant at all — the caller
/// answers `404 API_KEY_NOT_FOUND` either for a truly unknown id or one that
/// belongs to somebody else, on purpose (`api_keys.feature`: "A merchant
/// cannot revoke another merchant's key"). `Some(key_hash)` otherwise
/// (idempotent: revoking an already-revoked key of the caller's own
/// merchant still answers with its hash, so the cache entry is invalidated
/// again rather than the caller being told it never existed).
pub async fn revoke_api_key(
    db: &Db,
    merchant_id: MerchantId,
    key_id: i64,
) -> Result<Option<String>> {
    let row = sqlx::query(
        "UPDATE api_keys SET revoked_at = COALESCE(revoked_at, now()) \
          WHERE id = $1 AND merchant_id = $2 \
          RETURNING key_hash",
    )
    .bind(key_id)
    .bind(merchant_id)
    .fetch_optional(&db.0)
    .await?;
    match row {
        Some(row) => Ok(Some(row.try_get("key_hash")?)),
        None => Ok(None),
    }
}

#[derive(Debug, Clone)]
pub struct DashboardUserRow {
    pub merchant_id: MerchantId,
    pub email: String,
    pub password_hash: String,
}

pub async fn find_dashboard_user_by_email(
    db: &Db,
    email: &str,
) -> Result<Option<DashboardUserRow>> {
    let row = sqlx::query(
        "SELECT merchant_id, email, password_hash FROM dashboard_users WHERE email = $1",
    )
    .bind(email)
    .fetch_optional(&db.0)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(DashboardUserRow {
        merchant_id: row.try_get("merchant_id")?,
        email: row.try_get("email")?,
        password_hash: row.try_get("password_hash")?,
    }))
}
