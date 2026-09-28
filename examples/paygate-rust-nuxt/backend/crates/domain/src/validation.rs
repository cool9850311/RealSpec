//! Shape-level validation of a create-order request: `spec/openapi/openapi.yaml`'s
//! `CreateOrderRequest` patterns and bounds, as pure checks against fields
//! already parsed by serde. Deliberately does **not** know about a merchant's
//! own settlement currency (`CURRENCY_NOT_SUPPORTED`) — that needs the
//! merchant's row, which is not a fact this crate is allowed to depend on.

use crate::error::ValidationError;
use crate::ids::Amount;
use serde::{Deserialize, Serialize};

pub const MIN_AMOUNT: Amount = 1;
pub const MAX_AMOUNT: Amount = 99_999_999;
const MAX_TRADE_NO_LEN: usize = 20;
const MAX_ITEM_DESC_LEN: usize = 200;
const MAX_PATH_LEN: usize = 200;

/// The merchant-supplied half of `POST /payments`, exactly
/// `CreateOrderRequest` in `openapi.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateOrderRequest {
    pub merchant_trade_no: String,
    pub amount: Amount,
    pub currency: String,
    pub item_desc: String,
    pub notify_url: String,
    pub client_back_url: String,
}

fn is_merchant_trade_no(s: &str) -> bool {
    let len = s.chars().count();
    (1..=MAX_TRADE_NO_LEN).contains(&len)
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn is_currency(s: &str) -> bool {
    s.chars().count() == 3 && s.chars().all(|c| c.is_ascii_uppercase())
}

fn is_item_desc(s: &str) -> bool {
    let len = s.chars().count();
    (1..=MAX_ITEM_DESC_LEN).contains(&len)
}

/// `^/[^\s]*$`, `maxLength: 200` — an absolute path, with no whitespace, on
/// the same reverse proxy as everything else in this example (spec.md,
/// "Telling the merchant").
fn is_path(s: &str) -> bool {
    s.chars().count() <= MAX_PATH_LEN && s.starts_with('/') && !s.chars().any(|c| c.is_whitespace())
}

/// Checked in the order the OpenAPI document lists the fields, so that a
/// request breaking more than one rule at once still reports a stable,
/// predictable `field`.
pub fn validate_create_order(req: &CreateOrderRequest) -> Result<(), ValidationError> {
    if !is_merchant_trade_no(&req.merchant_trade_no) {
        return Err(ValidationError::new(
            "merchant_trade_no",
            "must be 1-20 characters of [A-Za-z0-9-]",
        ));
    }
    if !(MIN_AMOUNT..=MAX_AMOUNT).contains(&req.amount) {
        return Err(ValidationError::new(
            "amount",
            format!("must be between {MIN_AMOUNT} and {MAX_AMOUNT} minor units"),
        ));
    }
    if !is_currency(&req.currency) {
        return Err(ValidationError::new(
            "currency",
            "must be three uppercase letters",
        ));
    }
    if !is_item_desc(&req.item_desc) {
        return Err(ValidationError::new(
            "item_desc",
            format!("must be 1-{MAX_ITEM_DESC_LEN} characters"),
        ));
    }
    if !is_path(&req.notify_url) {
        return Err(ValidationError::new(
            "notify_url",
            "must be an absolute path with no whitespace",
        ));
    }
    if !is_path(&req.client_back_url) {
        return Err(ValidationError::new(
            "client_back_url",
            "must be an absolute path with no whitespace",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> CreateOrderRequest {
        CreateOrderRequest {
            merchant_trade_no: "ACME-0001".to_string(),
            amount: 1000,
            currency: "USD".to_string(),
            item_desc: "Beans".to_string(),
            notify_url: "/demo-merchant/api/notify".to_string(),
            client_back_url: "/shop/result".to_string(),
        }
    }

    #[test]
    fn a_well_formed_request_passes() {
        assert_eq!(validate_create_order(&valid()), Ok(()));
    }

    #[test]
    fn an_empty_trade_no_is_refused() {
        let mut r = valid();
        r.merchant_trade_no = "".to_string();
        assert_eq!(
            validate_create_order(&r).unwrap_err().field,
            "merchant_trade_no"
        );
    }

    #[test]
    fn a_trade_no_over_twenty_characters_is_refused() {
        let mut r = valid();
        r.merchant_trade_no = "ACME-0001-TOO-LONG-FOR-THE-LIMIT".to_string();
        assert_eq!(
            validate_create_order(&r).unwrap_err().field,
            "merchant_trade_no"
        );
    }

    #[test]
    fn a_trade_no_with_a_slash_is_refused() {
        let mut r = valid();
        r.merchant_trade_no = "ACME/0001".to_string();
        assert_eq!(
            validate_create_order(&r).unwrap_err().field,
            "merchant_trade_no"
        );
    }

    #[test]
    fn amount_at_either_edge_is_accepted() {
        let mut r = valid();
        r.amount = MIN_AMOUNT;
        assert_eq!(validate_create_order(&r), Ok(()));
        r.amount = MAX_AMOUNT;
        assert_eq!(validate_create_order(&r), Ok(()));
    }

    #[test]
    fn amount_outside_the_range_is_refused() {
        let mut r = valid();
        r.amount = 0;
        assert_eq!(validate_create_order(&r).unwrap_err().field, "amount");
        r.amount = MAX_AMOUNT + 1;
        assert_eq!(validate_create_order(&r).unwrap_err().field, "amount");
        r.amount = -5;
        assert_eq!(validate_create_order(&r).unwrap_err().field, "amount");
    }

    #[test]
    fn lowercase_currency_is_refused() {
        let mut r = valid();
        r.currency = "usd".to_string();
        assert_eq!(validate_create_order(&r).unwrap_err().field, "currency");
    }

    #[test]
    fn an_empty_item_desc_is_refused() {
        let mut r = valid();
        r.item_desc = "".to_string();
        assert_eq!(validate_create_order(&r).unwrap_err().field, "item_desc");
    }

    #[test]
    fn a_notify_url_that_is_not_a_path_is_refused() {
        let mut r = valid();
        r.notify_url = "not-a-url".to_string();
        assert_eq!(validate_create_order(&r).unwrap_err().field, "notify_url");
    }

    #[test]
    fn a_client_back_url_that_is_not_a_path_is_refused() {
        let mut r = valid();
        r.client_back_url = "not-a-url".to_string();
        assert_eq!(
            validate_create_order(&r).unwrap_err().field,
            "client_back_url"
        );
    }

    #[test]
    fn a_path_with_whitespace_is_refused() {
        let mut r = valid();
        r.notify_url = "/has space".to_string();
        assert_eq!(validate_create_order(&r).unwrap_err().field, "notify_url");
    }
}
