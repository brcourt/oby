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
}
