//! The idempotency fingerprint: a stable hash of "this request", used to
//! tell a genuine retry (same key, same body) from a key reused for a
//! different request. Stability across JSON key ordering is the whole point
//! — two requests whose bodies differ only in field order must fingerprint
//! identically, so the canonical form sorts object keys recursively rather
//! than trusting the order serde happened to parse them in.

use crate::ids::MerchantId;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// A stable hash of the semantically significant request fields: the
/// merchant (so one key means different things to different merchants) and
/// the body, canonicalised so key order never matters.
pub fn idempotency_fingerprint(merchant_id: MerchantId, body: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(merchant_id.to_string().as_bytes());
    hasher.update(b":");
    hasher.update(canonicalize(body).as_bytes());
    hex::encode(hasher.finalize())
}

/// A deterministic text form of a JSON value: object keys sorted, everything
/// else serialized as JSON would. Never panics — the caller may be on a
/// request path, and a fingerprint that degrades gracefully on a value
/// `serde_json` still refuses to print (impossible for `Value` in practice,
/// but not a place to introduce a panic) is better than one that crashes it.
fn canonicalize(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    let key = serde_json::to_string(k).unwrap_or_default();
                    format!("{key}:{}", canonicalize(&map[k]))
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(canonicalize).collect();
            format!("[{}]", parts.join(","))
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stable_across_key_ordering_in_the_body() {
        let a = json!({ "amount": 1000, "merchant_trade_no": "ACME-1" });
        let b = json!({ "merchant_trade_no": "ACME-1", "amount": 1000 });
        assert_eq!(
            idempotency_fingerprint(1, &a),
            idempotency_fingerprint(1, &b)
        );
    }

    #[test]
    fn stable_across_nested_key_ordering_too() {
        let a = json!({ "amount": 1000, "meta": { "a": 1, "b": 2 } });
        let b = json!({ "meta": { "b": 2, "a": 1 }, "amount": 1000 });
        assert_eq!(
            idempotency_fingerprint(1, &a),
            idempotency_fingerprint(1, &b)
        );
    }

    #[test]
    fn different_merchants_fingerprint_the_same_body_differently() {
        let body = json!({ "amount": 1000 });
        assert_ne!(
            idempotency_fingerprint(1, &body),
            idempotency_fingerprint(2, &body)
        );
    }

    #[test]
    fn a_different_value_fingerprints_differently() {
        let a = json!({ "amount": 1000 });
        let b = json!({ "amount": 1001 });
        assert_ne!(
            idempotency_fingerprint(1, &a),
            idempotency_fingerprint(1, &b)
        );
    }

    #[test]
    fn array_element_order_is_significant() {
        // Unlike object keys, array order is semantically meaningful and
        // must NOT be normalised away.
        let a = json!({ "items": [1, 2] });
        let b = json!({ "items": [2, 1] });
        assert_ne!(
            idempotency_fingerprint(1, &a),
            idempotency_fingerprint(1, &b)
        );
    }

    #[test]
    fn is_a_lowercase_hex_sha256_digest() {
        let fp = idempotency_fingerprint(1, &json!({}));
        assert_eq!(fp.len(), 64);
        assert!(fp
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }
}
