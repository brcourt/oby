//! Per-filter arg parsing, config, and pipeline building for the
//! pre-filter tee branches injected by `rewrite::inject_pre_filter_tees`.

// Tasks 1.2–1.4 and 2.1 will use all items in this module; suppress
// dead-code lints until the wiring is complete.
#![allow(dead_code)]

/// Window sizes for the pre-filter tee branches. Populated from env vars
/// at PreToolUse hook invocation time; embedded as literal numbers in
/// the rewritten command so there's no env-var indirection at shell
/// execution time.
#[derive(Debug, PartialEq, Eq)]
pub struct PreFilterConfig {
    pub grep_before: usize,
    pub grep_after: usize,
    pub head_peek: usize,
    pub tail_peek: usize,
}

impl PreFilterConfig {
    /// Read the four `OBS_*_LINES` env vars; missing / empty / unparseable
    /// values fall back to 3. Values are clamped to 0..=1000 to bound the
    /// activity feed against accidental misconfiguration.
    pub fn from_env() -> Self {
        Self {
            grep_before: read_env("OBS_GREP_BEFORE_LINES"),
            grep_after: read_env("OBS_GREP_AFTER_LINES"),
            head_peek: read_env("OBS_HEAD_PEEK_LINES"),
            tail_peek: read_env("OBS_TAIL_PEEK_LINES"),
        }
    }
}

/// Return true when the user's grep args include `-v` / `--invert-match`
/// in any tokenized form.
///
/// Detection is approximate by design — naive whitespace tokenization,
/// no quote handling. False positives in pathological inputs (`grep -- -v`,
/// patterns that contain the literal substring `--invert-match`) are
/// documented as known limitations in `docs/specs/2026-06-09-v0.2.2-design.md`.
/// The worst-case impact is a missing pre-grep chunk, not a broken rewrite.
pub fn grep_is_invert_match(args: &str) -> bool {
    for tok in args.split_whitespace() {
        if tok == "--invert-match" {
            return true;
        }
        // Short-flag run: starts with single `-`, then ASCII letters only.
        // The `--` case is handled above; we don't want to misread it as a
        // short flag.
        if let Some(rest) = tok.strip_prefix('-') {
            if !rest.starts_with('-')
                && rest.chars().all(|c| c.is_ascii_alphabetic())
                && rest.contains('v')
            {
                return true;
            }
        }
    }
    false
}

fn read_env(name: &str) -> usize {
    const DEFAULT: usize = 3;
    const MAX: usize = 1000;
    match std::env::var(name) {
        Ok(s) => s
            .trim()
            .parse::<usize>()
            .map(|n| n.min(MAX))
            .unwrap_or(DEFAULT),
        Err(_) => DEFAULT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_env<F: FnOnce()>(vars: &[(&str, Option<&str>)], f: F) {
        // Snapshot prior values so the test cleans up after itself.
        let snapshot: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(k, _)| (k.to_string(), std::env::var(k).ok()))
            .collect();
        for (k, v) in vars {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
        f();
        for (k, prior) in snapshot {
            match prior {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
    }

    #[test]
    fn defaults_when_unset() {
        with_env(
            &[
                ("OBS_GREP_BEFORE_LINES", None),
                ("OBS_GREP_AFTER_LINES", None),
                ("OBS_HEAD_PEEK_LINES", None),
                ("OBS_TAIL_PEEK_LINES", None),
            ],
            || {
                let cfg = PreFilterConfig::from_env();
                assert_eq!(cfg.grep_before, 3);
                assert_eq!(cfg.grep_after, 3);
                assert_eq!(cfg.head_peek, 3);
                assert_eq!(cfg.tail_peek, 3);
            },
        );
    }

    #[test]
    fn parses_valid_values() {
        with_env(
            &[
                ("OBS_GREP_BEFORE_LINES", Some("5")),
                ("OBS_GREP_AFTER_LINES", Some("7")),
                ("OBS_HEAD_PEEK_LINES", Some("10")),
                ("OBS_TAIL_PEEK_LINES", Some("0")),
            ],
            || {
                let cfg = PreFilterConfig::from_env();
                assert_eq!(cfg.grep_before, 5);
                assert_eq!(cfg.grep_after, 7);
                assert_eq!(cfg.head_peek, 10);
                assert_eq!(cfg.tail_peek, 0);
            },
        );
    }

    #[test]
    fn clamps_above_max() {
        with_env(&[("OBS_GREP_BEFORE_LINES", Some("9999"))], || {
            let cfg = PreFilterConfig::from_env();
            assert_eq!(cfg.grep_before, 1000);
        });
    }

    #[test]
    fn falls_back_on_garbage() {
        with_env(&[("OBS_GREP_AFTER_LINES", Some("garbage"))], || {
            let cfg = PreFilterConfig::from_env();
            assert_eq!(cfg.grep_after, 3);
        });
    }

    #[test]
    fn falls_back_on_empty() {
        with_env(&[("OBS_HEAD_PEEK_LINES", Some(""))], || {
            let cfg = PreFilterConfig::from_env();
            assert_eq!(cfg.head_peek, 3);
        });
    }

    #[test]
    fn trims_whitespace_before_parse() {
        with_env(&[("OBS_TAIL_PEEK_LINES", Some("  4  "))], || {
            let cfg = PreFilterConfig::from_env();
            assert_eq!(cfg.tail_peek, 4);
        });
    }

    #[test]
    fn grep_invert_false_for_plain_pattern() {
        assert!(!grep_is_invert_match("foo"));
    }

    #[test]
    fn grep_invert_false_for_other_flags() {
        assert!(!grep_is_invert_match("-i foo"));
        assert!(!grep_is_invert_match("-E '^[a-z]+$'"));
        assert!(!grep_is_invert_match("-rn foo dir/"));
    }

    #[test]
    fn grep_invert_true_for_standalone_v() {
        assert!(grep_is_invert_match("-v foo"));
    }

    #[test]
    fn grep_invert_true_for_v_at_end_of_args() {
        // User-tail position is still detected.
        assert!(grep_is_invert_match("foo -v"));
    }

    #[test]
    fn grep_invert_true_for_combined_short_flags_with_v() {
        // grep -vi PATTERN, grep -vE PATTERN, grep -vIn PATTERN — all count.
        assert!(grep_is_invert_match("-vi foo"));
        assert!(grep_is_invert_match("-vE foo"));
        assert!(grep_is_invert_match("-vIn foo"));
        assert!(grep_is_invert_match("-iv foo"));
    }

    #[test]
    fn grep_invert_true_for_long_form() {
        assert!(grep_is_invert_match("--invert-match foo"));
        assert!(grep_is_invert_match("foo --invert-match"));
    }

    #[test]
    fn grep_invert_false_for_other_long_flags() {
        assert!(!grep_is_invert_match("--color=always foo"));
        assert!(!grep_is_invert_match("--include='*.rs' foo"));
    }

    #[test]
    fn grep_invert_false_for_long_form_substring_in_other_flag() {
        // --invert-match must be the exact long-form token; partial matches
        // inside other args don't trigger.
        assert!(!grep_is_invert_match("--invert-something foo"));
    }
}
