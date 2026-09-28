//! `Set-Cookie` string building for the dashboard's session pair. Kept in
//! one place because both halves of the pair (`session`, `session_hint`) and
//! both directions (mint, clear) need `COOKIE_SECURE` applied identically
//! (`spec.md`, "Environment variables": `COOKIE_SECURE` is the setting
//! that decides whether every cookie this module builds carries `Secure`).

pub fn session_cookie(token: &str, ttl_secs: u64, secure: bool) -> String {
    let mut s = format!("session={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={ttl_secs}");
    if secure {
        s.push_str("; Secure");
    }
    s
}

pub fn session_hint_cookie(ttl_secs: u64, secure: bool) -> String {
    let mut s = format!("session_hint=1; Path=/; SameSite=Lax; Max-Age={ttl_secs}");
    if secure {
        s.push_str("; Secure");
    }
    s
}

/// A logout sets the session cookie to empty with `Max-Age=0`, which every
/// browser treats as "delete immediately".
pub fn clear_session_cookie(secure: bool) -> String {
    let mut s = "session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0".to_string();
    if secure {
        s.push_str("; Secure");
    }
    s
}

pub fn clear_session_hint_cookie(secure: bool) -> String {
    let mut s = "session_hint=; Path=/; SameSite=Lax; Max-Age=0".to_string();
    if secure {
        s.push_str("; Secure");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_cookie_is_httponly_and_carries_the_token() {
        let c = session_cookie("tok123", 28800, false);
        assert!(c.starts_with("session=tok123;"));
        assert!(c.contains("HttpOnly"));
        assert!(c.contains("SameSite=Lax"));
        assert!(c.contains("Max-Age=28800"));
        assert!(!c.contains("Secure"));
    }

    #[test]
    fn cookie_secure_true_appends_secure_to_every_cookie_this_module_builds() {
        assert!(session_cookie("t", 1, true).ends_with("; Secure"));
        assert!(session_hint_cookie(1, true).ends_with("; Secure"));
        assert!(clear_session_cookie(true).ends_with("; Secure"));
        assert!(clear_session_hint_cookie(true).ends_with("; Secure"));
    }

    #[test]
    fn cookie_secure_false_never_adds_secure_to_any_cookie_this_module_builds() {
        assert!(!session_cookie("t", 1, false).contains("Secure"));
        assert!(!session_hint_cookie(1, false).contains("Secure"));
        assert!(!clear_session_cookie(false).contains("Secure"));
        assert!(!clear_session_hint_cookie(false).contains("Secure"));
    }

    #[test]
    fn the_session_hint_cookie_is_readable_not_httponly() {
        let c = session_hint_cookie(28800, false);
        assert!(c.starts_with("session_hint=1;"));
        assert!(!c.contains("HttpOnly"));
    }

    #[test]
    fn a_logout_sets_the_session_cookie_to_empty_with_max_age_zero() {
        let c = clear_session_cookie(false);
        assert!(c.starts_with("session=;"));
        assert!(c.contains("Max-Age=0"));
        assert!(c.contains("HttpOnly"));

        let hint = clear_session_hint_cookie(false);
        assert!(hint.starts_with("session_hint=;"));
        assert!(hint.contains("Max-Age=0"));
    }
}
