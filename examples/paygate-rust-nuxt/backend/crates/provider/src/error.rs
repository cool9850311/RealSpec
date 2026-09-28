//! Everything an adapter can refuse to do. Every variant here is something a
//! caller can turn into a `400` (a callback/query that did not verify) or a
//! `502` (a malformed answer from the provider) without inspecting a string.

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    /// `CheckMacValue` / `TradeSha` did not verify, or verified against the
    /// wrong key pair. Constant-time compared — see `spec.md`, "Authentication".
    #[error("signature did not verify")]
    InvalidSignature,

    /// A field the wire format requires was not present at all.
    #[error("missing required field {0}")]
    MissingField(&'static str),

    /// A field was present but not shaped the way it needed to be (not an
    /// integer, not valid hex, not a recognised value).
    #[error("malformed value for field {0}: {1:?}")]
    MalformedField(&'static str, String),

    /// `TradeInfo` would not decrypt: wrong key/IV, truncated, or not valid
    /// AES-256-CBC/PKCS7 ciphertext.
    #[error("TradeInfo could not be decrypted")]
    DecryptionFailed,

    /// A signed answer's own `TimeStamp` fell outside the window it is valid
    /// for — a clock problem, or a replay.
    #[error("TimeStamp is outside the valid window")]
    TimestampOutOfRange,

    /// The response body was not shaped like any answer this adapter knows
    /// how to read.
    #[error("could not parse the provider's response")]
    MalformedResponse,
}
