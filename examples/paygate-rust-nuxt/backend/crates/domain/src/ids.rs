//! Identifiers and money. `Amount` is always minor units (cents, or the
//! equivalent for the currency), matching every provider's own wire format.

/// A merchant's numeric id in `merchants.id`.
pub type MerchantId = i64;

/// Minor units of currency. Always a whole number: ECPay and NewebPay both
/// speak integer minor units, and so does every table in the schema.
pub type Amount = i64;
