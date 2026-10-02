//! Parses `spec/bdd/format.yml` well enough to check registry parity, without
//! pulling in a YAML crate: the file's shape is simple enough (one flat list
//! of steps, each a small set of scalar/`[...]` fields) that a few
//! fixed-indentation line scans give us everything we need — `id`, `keywords`
//! and the step-level `pattern` (never the nested `captures[].pattern`, which
//! lives at a deeper indentation and is skipped on purpose).
//!
//! This module is exercised only by `#[test]`s (see the bottom of this file)
//! run with `cargo test`, which need no containers.

use std::{fs, path::PathBuf};

/// One step definition as declared in `format.yml`.
#[derive(Debug, Clone)]
pub struct StepDef {
    pub id: String,
    pub keywords: Vec<String>,
    pub pattern: String,
}

/// `$ROOT`, resolved from this crate's own manifest directory.
pub fn spec_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn format_yml_path() -> PathBuf {
    spec_root().join("spec/bdd/format.yml")
}

pub fn api_features_dir() -> PathBuf {
    spec_root().join("spec/bdd/api")
}

/// Extracts the double-quoted scalar starting right after `prefix` on `line`,
/// applying YAML double-quote escaping as it goes: `\"` is a literal quote
/// (not the closing delimiter — see e.g. `response_body_does_not_contain`),
/// and `\\` is a literal backslash (needed by every e2e pattern's `\.`, `\{`
/// and `\}`, written in the source as `\\.`, `\\{`, `\\}`). Any other escape
/// is passed through as the character it names, which is enough for every
/// pattern in this file.
fn extract_dq_after(line: &str, prefix: &str) -> Option<String> {
    let rest = line.strip_prefix(prefix)?;
    let bytes = rest.as_bytes();
    let mut i = 0;
    let mut out = String::new();
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => {
                out.push(bytes[i + 1] as char);
                i += 2;
            }
            b'"' => return Some(out),
            b => {
                out.push(b as char);
                i += 1;
            }
        }
    }
    None
}

/// Parses every step definition out of `format.yml`.
///
/// Only the top-level `steps:` list is considered; `id` lines are found at
/// two-space indentation (`  - id: foo`), and `keywords:`/`pattern:` belonging
/// to that step are found at exactly four-space indentation, which is what
/// keeps a nested `captures[].pattern:` (eight spaces) from being mistaken for
/// the step's own pattern.
pub fn parse_format_yml() -> Vec<StepDef> {
    let text = fs::read_to_string(format_yml_path())
        .unwrap_or_else(|e| panic!("reading {:?}: {e}", format_yml_path()));

    let mut steps = Vec::new();
    let mut current: Option<(String, Vec<String>, Option<String>)> = None;

    for raw_line in text.lines() {
        if let Some(id) = raw_line.strip_prefix("  - id: ") {
            if let Some((id, keywords, pattern)) = current.take() {
                steps.push(StepDef {
                    id,
                    keywords,
                    pattern: pattern.unwrap_or_default(),
                });
            }
            current = Some((id.trim().to_string(), Vec::new(), None));
            continue;
        }
        let Some((_, keywords, pattern)) = current.as_mut() else {
            continue;
        };
        if let Some(rest) = raw_line.strip_prefix("    keywords: [") {
            let list = rest.trim_end_matches(']').trim_end_matches("] ");
            *keywords = list
                .split(',')
                .map(|s| s.trim().trim_end_matches(']').to_string())
                .filter(|s| !s.is_empty())
                .collect();
            continue;
        }
        if raw_line.starts_with("    pattern: \"") {
            if let Some(p) = extract_dq_after(raw_line, "    pattern: \"") {
                *pattern = Some(p);
            }
            continue;
        }
    }
    if let Some((id, keywords, pattern)) = current.take() {
        steps.push(StepDef {
            id,
            keywords,
            pattern: pattern.unwrap_or_default(),
        });
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steps::IMPLEMENTED_STEPS;
    use regex::Regex;

    /// Every step this crate implements is declared in `format.yml` with the
    /// byte-identical pattern.
    #[test]
    fn implemented_steps_match_registry_verbatim() {
        let registry = parse_format_yml();
        for (id, pattern) in IMPLEMENTED_STEPS {
            let def = registry
                .iter()
                .find(|s| s.id == *id)
                .unwrap_or_else(|| panic!("format.yml has no step id {id:?}"));
            assert_eq!(
                &def.pattern, pattern,
                "step {id:?}: implemented pattern does not match format.yml verbatim"
            );
        }
    }

    /// Every step must be registered for `Given`, `When` AND `Then`.
    ///
    /// `format.yml` says it in as many words: "Cucumber step definitions are
    /// keyword-agnostic, so the `keywords` field has no effect when the tests
    /// run." cucumber-rs is not
    /// keyword-agnostic by itself — `#[when]` registers for `When` only — and
    /// `And`/`But` inherit the keyword of the step above them. So a step written
    /// `And the customer pays at the payment provider:` after a `Then` is looked
    /// up as a *Then*, and a `#[when]`-only definition does not match it.
    ///
    /// That is not a hypothetical: it took out every scenario in
    /// `refunds.feature` at once, with `Step doesn't match any function` — a
    /// message that reads like a missing implementation when the implementation
    /// was right there. Seventeen of the twenty-five steps were registered for
    /// one or two keywords, so any feature reaching them through `And` after a
    /// different keyword failed.
    ///
    /// This reads the source rather than the registry, because what it checks is
    /// a property of the bindings and not of the specification. It counts
    /// parentheses rather than matching lines, because `cargo fmt` wraps a long
    /// attribute across several lines and a line-based scan silently sees those
    /// as separate groups — which is how the first version of this very test
    /// passed while the bug was still there.
    #[test]
    fn every_step_is_registered_for_all_three_keywords() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/steps");
        let mut offenders = Vec::new();

        for entry in std::fs::read_dir(&dir).expect("reading src/steps") {
            let path = entry.expect("a directory entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("reading a step module");
            let file = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string();

            // Every step attribute, with the byte range it actually occupies.
            let mut attrs: Vec<(usize, usize, &str)> = Vec::new();
            for keyword in ["given", "when", "then"] {
                let marker = format!("\n#[{keyword}(");
                let mut from = 0;
                while let Some(found) = source[from..].find(&marker) {
                    let open = from + found + marker.len() - 1; // at '('
                    let mut depth = 0usize;
                    let mut i = open;
                    for (offset, ch) in source[open..].char_indices() {
                        match ch {
                            '(' => depth += 1,
                            ')' => {
                                depth -= 1;
                                if depth == 0 {
                                    i = open + offset;
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    attrs.push((open + 1 - marker.len(), i + 2, keyword));
                    from = i + 2;
                }
            }
            attrs.sort_by_key(|a| a.0);

            // Group the ones with nothing but whitespace between them.
            let mut group: Vec<(usize, usize, &str)> = Vec::new();
            let mut groups: Vec<Vec<(usize, usize, &str)>> = Vec::new();
            for attr in attrs {
                match group.last() {
                    Some(previous) if source[previous.1..attr.0].trim().is_empty() => {
                        group.push(attr)
                    }
                    Some(_) => {
                        groups.push(std::mem::take(&mut group));
                        group.push(attr);
                    }
                    None => group.push(attr),
                }
            }
            if !group.is_empty() {
                groups.push(group);
            }

            for group in groups {
                let name = source[group[group.len() - 1].1..]
                    .split("fn ")
                    .nth(1)
                    .and_then(|rest| rest.split('(').next())
                    .unwrap_or("<unknown>")
                    .trim()
                    .to_string();
                for keyword in ["given", "when", "then"] {
                    if !group.iter().any(|attr| attr.2 == keyword) {
                        offenders.push(format!("{file}: {name} is not registered for {keyword}"));
                    }
                }
            }
        }

        assert!(
            offenders.is_empty(),
            "every step definition must carry #[given], #[when] and #[then] with the same \
             regex, because `And` inherits the keyword above it:\n  {}",
            offenders.join("\n  ")
        );
    }

    /// Every sentence a race may name is a real step of `format.yml`, declared
    /// with `When` — the rule `step_ref` states there. The registry says what
    /// may be NAMED; `steps::http::RACEABLE_STEPS` says what this harness can
    /// run concurrently, and this is what keeps the second from drifting away
    /// from the first.
    #[test]
    fn every_raceable_sentence_is_a_when_step_of_the_registry() {
        let registry = parse_format_yml();
        for sentence in crate::steps::http::race_tests::RACEABLE_SAMPLES {
            let def = registry
                .iter()
                .find(|s| {
                    Regex::new(&s.pattern)
                        .map(|re| re.is_match(sentence))
                        .unwrap_or(false)
                })
                .unwrap_or_else(|| {
                    panic!("a race may name {sentence:?}, which matches no step in format.yml")
                });
            assert!(
                def.keywords.iter().any(|k| k == "When"),
                "a race may name {sentence:?} (step {:?}), which format.yml does not declare with \
                 `When`: only an action can take part in a race",
                def.id
            );
        }
    }
}
