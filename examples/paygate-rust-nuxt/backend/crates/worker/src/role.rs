//! `paygate-worker <role>` argument parsing. A pure function, tested without
//! touching `std::env::args` (`main.rs` is the only caller that reaches for
//! the real process arguments): role parsing and dispatch get their own
//! unit tests like every other crate here.

use std::fmt;

/// One of the five roles `spec.md`'s binary/role table names. `RebuildReports`
/// is spelled as two command-line words (`rebuild reports`), matching how
/// `backend/apitest/src/stack.rs` invokes the one-shot container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Relay,
    Ingest,
    Notify,
    Reconcile,
    RebuildReports,
}

impl Role {
    /// The role's own name, as `GET /status` reports it and as every log line
    /// naming the role spells it.
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Relay => "relay",
            Role::Ingest => "ingest",
            Role::Notify => "notify",
            Role::Reconcile => "reconcile",
            Role::RebuildReports => "rebuild reports",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parses the process's own argument list (everything after the binary name)
/// into a [`Role`], or a message naming what was wrong — the same rule that
/// startup fails loudly, naming what is bad, applies to a bad role exactly
/// as it does to a bad environment variable.
pub fn parse_role(args: &[String]) -> Result<Role, String> {
    match args {
        [a] if a == "relay" => Ok(Role::Relay),
        [a] if a == "ingest" => Ok(Role::Ingest),
        [a] if a == "notify" => Ok(Role::Notify),
        [a] if a == "reconcile" => Ok(Role::Reconcile),
        [a, b] if a == "rebuild" && b == "reports" => Ok(Role::RebuildReports),
        [] => Err(
            "missing role: expected one of relay | ingest | notify | reconcile | rebuild reports"
                .to_string(),
        ),
        other => Err(format!(
            "unknown role {:?}: expected one of relay | ingest | notify | reconcile | rebuild reports",
            other.join(" ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn each_single_word_role_parses() {
        assert_eq!(parse_role(&args(&["relay"])), Ok(Role::Relay));
        assert_eq!(parse_role(&args(&["ingest"])), Ok(Role::Ingest));
        assert_eq!(parse_role(&args(&["notify"])), Ok(Role::Notify));
        assert_eq!(parse_role(&args(&["reconcile"])), Ok(Role::Reconcile));
    }

    #[test]
    fn rebuild_reports_is_two_words() {
        assert_eq!(
            parse_role(&args(&["rebuild", "reports"])),
            Ok(Role::RebuildReports)
        );
    }

    #[test]
    fn no_arguments_is_an_error_naming_the_choices() {
        let err = parse_role(&args(&[])).unwrap_err();
        assert!(err.contains("missing role"));
    }

    #[test]
    fn an_unknown_word_is_an_error() {
        let err = parse_role(&args(&["bogus"])).unwrap_err();
        assert!(err.contains("bogus"));
    }

    #[test]
    fn rebuild_alone_is_not_a_role() {
        assert!(parse_role(&args(&["rebuild"])).is_err());
    }

    #[test]
    fn extra_arguments_are_rejected() {
        assert!(parse_role(&args(&["relay", "extra"])).is_err());
        assert!(parse_role(&args(&["rebuild", "reports", "extra"])).is_err());
    }

    #[test]
    fn role_display_matches_the_documented_names() {
        assert_eq!(Role::Relay.to_string(), "relay");
        assert_eq!(Role::RebuildReports.to_string(), "rebuild reports");
    }
}
