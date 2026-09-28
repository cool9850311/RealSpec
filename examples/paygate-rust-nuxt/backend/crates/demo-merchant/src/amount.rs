//! Converting the shop's decimal string (`"12.50"`) into paygate's own
//! convention, an integer count of minor units (`1250`).
//!
//! `spec/openapi/demo-merchant.yaml`'s `POST /orders` pins the wire shape with
//! a regex: `^[0-9]+(\.[0-9]{1,2})?$` — a non-negative integer, optionally
//! followed by one or two fractional digits. This is validated and converted
//! by hand, in integer arithmetic throughout, so that a value like `"0.1"`
//! never round-trips through a `f64` on its way to money.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidAmount;

impl fmt::Display for InvalidAmount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "amount does not match ^[0-9]+(\\.[0-9]{{1,2}})?$")
    }
}

impl std::error::Error for InvalidAmount {}

/// Parses a decimal string into minor units. Rejects anything that is not
/// `^[0-9]+(\.[0-9]{1,2})?$` — including a leading `+`/`-`, more than two
/// fractional digits, or an empty string — and rejects a value that would not
/// fit in an `i64` once scaled by 100.
pub fn parse_decimal_to_minor_units(input: &str) -> Result<i64, InvalidAmount> {
    if input.is_empty() {
        return Err(InvalidAmount);
    }

    let mut parts = input.splitn(2, '.');
    let whole = parts.next().ok_or(InvalidAmount)?;
    let frac = parts.next();

    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return Err(InvalidAmount);
    }

    let frac_padded: String = match frac {
        None => "00".to_string(),
        Some(f) => {
            if f.is_empty() || f.len() > 2 || !f.bytes().all(|b| b.is_ascii_digit()) {
                return Err(InvalidAmount);
            }
            if f.len() == 1 {
                format!("{f}0")
            } else {
                f.to_string()
            }
        }
    };

    let whole_value: i64 = whole.parse().map_err(|_| InvalidAmount)?;
    let frac_value: i64 = frac_padded.parse().map_err(|_| InvalidAmount)?;

    whole_value
        .checked_mul(100)
        .and_then(|scaled| scaled.checked_add(frac_value))
        .ok_or(InvalidAmount)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_dollars_have_no_fraction() {
        assert_eq!(parse_decimal_to_minor_units("12"), Ok(1200));
    }

    #[test]
    fn two_fractional_digits() {
        assert_eq!(parse_decimal_to_minor_units("12.50"), Ok(1250));
    }

    #[test]
    fn one_fractional_digit_is_padded() {
        assert_eq!(parse_decimal_to_minor_units("12.5"), Ok(1250));
    }

    #[test]
    fn zero_dollars_and_cents() {
        assert_eq!(parse_decimal_to_minor_units("0.01"), Ok(1));
    }

    #[test]
    fn three_fractional_digits_is_rejected() {
        assert_eq!(parse_decimal_to_minor_units("12.500"), Err(InvalidAmount));
    }

    #[test]
    fn empty_string_is_rejected() {
        assert_eq!(parse_decimal_to_minor_units(""), Err(InvalidAmount));
    }

    #[test]
    fn a_bare_dot_is_rejected() {
        assert_eq!(parse_decimal_to_minor_units("12."), Err(InvalidAmount));
        assert_eq!(parse_decimal_to_minor_units(".50"), Err(InvalidAmount));
    }

    #[test]
    fn a_sign_is_rejected() {
        assert_eq!(parse_decimal_to_minor_units("-12.50"), Err(InvalidAmount));
        assert_eq!(parse_decimal_to_minor_units("+12.50"), Err(InvalidAmount));
    }

    #[test]
    fn non_digits_are_rejected() {
        assert_eq!(parse_decimal_to_minor_units("abc"), Err(InvalidAmount));
        assert_eq!(parse_decimal_to_minor_units("12.ab"), Err(InvalidAmount));
    }

    #[test]
    fn an_amount_too_large_to_scale_is_rejected() {
        assert_eq!(
            parse_decimal_to_minor_units("99999999999999999.99"),
            Err(InvalidAmount)
        );
    }
}
