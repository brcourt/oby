//! Detect which shell CC's Bash tool will execute the command in.

pub enum Shell {
    Bash,
    Zsh,
    Other,
}

pub fn detect() -> Shell {
    if std::env::var("BASH_VERSION").is_ok() {
        return Shell::Bash;
    }
    if std::env::var("ZSH_VERSION").is_ok() {
        return Shell::Zsh;
    }
    match std::env::var("SHELL").ok().as_deref() {
        Some(s) if s.ends_with("/bash") => Shell::Bash,
        Some(s) if s.ends_with("/zsh") => Shell::Zsh,
        _ => Shell::Other,
    }
}

impl Shell {
    pub fn supports_process_substitution(&self) -> bool {
        matches!(self, Shell::Bash | Shell::Zsh)
    }

    /// Build the per-shell xtrace wrap blueprint. The caller passes the
    /// already-formatted oby-tee sink invocations so they stay in lockstep
    /// with the stdout sink built by `rewrite()`.
    pub fn xtrace_wrap(&self, stderr_sink: &str, xtrace_sink: &str) -> XtraceWrap {
        match self {
            Shell::Bash => XtraceWrap {
                prefix: "BASH_XTRACEFD=9\nset -x\n".to_string(),
                stderr_redirect: format!("2> >(tee >({stderr_sink} >/dev/null) >&2)"),
                xtrace_fds: format!("9> >({xtrace_sink} >/dev/null)"),
            },
            Shell::Zsh => {
                // PS4 sentinel: \x01 is a control byte unlikely to appear in
                // user stderr. zsh expands %N (current source) and %i (line)
                // so the trace stays zsh-native after the sentinel is stripped.
                //
                // awk demuxer:
                //   /^\x01/ → strip sentinel, write to FD 9 (xtrace)
                //   else    → write to FD 8 (stderr capture) AND FD 7 (real stderr → agent)
                //
                // Both /dev/fd/N writes are buffered; fflush after each.
                let prefix = r#"PS4=$'\x01+(%N):%i> '
set -x
"#
                .to_string();
                let demuxer = r#"awk '/^\x01/ { sub(/^\x01/, ""); print > "/dev/fd/9"; fflush("/dev/fd/9"); next } { print > "/dev/fd/8"; fflush("/dev/fd/8"); print > "/dev/fd/7"; fflush("/dev/fd/7") }'"#;
                XtraceWrap {
                    prefix,
                    stderr_redirect: format!("2> >({demuxer})"),
                    xtrace_fds: format!(
                        "7>&2 8> >({stderr_sink} >/dev/null) 9> >({xtrace_sink} >/dev/null)"
                    ),
                }
            }
            Shell::Other => XtraceWrap {
                prefix: String::new(),
                stderr_redirect: format!("2> >(tee >({stderr_sink} >/dev/null) >&2)"),
                xtrace_fds: String::new(),
            },
        }
    }
}

/// Per-shell wrap blueprint for the outer xtrace + stdout/stderr layer.
///
/// The three fields are spliced into the format string in `rewrite()`:
///
/// ```text
/// { <prefix><inner>\n} > >(tee stdout) <stderr_redirect> <xtrace_fds>
/// ```
///
/// Each shell produces a different shape. The caller does NOT need to know
/// which shell is in play — it just stitches the three pieces together.
pub struct XtraceWrap {
    /// Inserted at the top of the brace block, before the inner command.
    /// Empty string if no xtrace is desired.
    pub prefix: String,
    /// The full `2> >(...)` redirect on the brace block. Always includes
    /// a stderr capture; under zsh, also includes the awk demuxer that
    /// splits sentinel-prefixed lines to FD 9.
    pub stderr_redirect: String,
    /// Trailing FD redirects on the brace block. Under bash this is
    /// `9> >(xtrace_sink)`; under zsh it's `7>&2 8> >(stderr_sink) 9> >(xtrace_sink)`.
    /// Empty string under shells that don't get xtrace.
    pub xtrace_fds: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_supports_process_sub() {
        assert!(Shell::Bash.supports_process_substitution());
    }

    #[test]
    fn zsh_supports_process_sub() {
        assert!(Shell::Zsh.supports_process_substitution());
    }

    #[test]
    fn other_does_not() {
        assert!(!Shell::Other.supports_process_substitution());
    }

    #[test]
    fn bash_xtrace_wrap_uses_bash_xtracefd() {
        let w = Shell::Bash.xtrace_wrap("STDERR_SINK", "XTRACE_SINK");
        assert!(w.prefix.contains("BASH_XTRACEFD=9"));
        assert!(w.prefix.contains("set -x"));
        // Stderr is the v0.1 form: simple tee with pass-through to FD 2.
        assert!(w.stderr_redirect.contains("2> >(tee"));
        assert!(w.stderr_redirect.contains(">&2"));
        // FD 9 carries xtrace.
        assert!(w.xtrace_fds.contains("9> >("));
    }

    #[test]
    fn zsh_xtrace_wrap_uses_ps4_sentinel_and_awk_demuxer() {
        let w = Shell::Zsh.xtrace_wrap("STDERR_SINK", "XTRACE_SINK");
        // PS4 prepends a control-byte sentinel so awk can split.
        assert!(
            w.prefix.contains(r"PS4=$'\x01"),
            "zsh prefix must set PS4 with \\x01 sentinel; got: {}",
            w.prefix
        );
        assert!(w.prefix.contains("set -x"));
        // Stderr is the awk demuxer.
        assert!(
            w.stderr_redirect.contains("awk"),
            "zsh stderr redirect must use awk demuxer; got: {}",
            w.stderr_redirect
        );
        // Three block-level FDs: 7 (real stderr passthrough), 8 (stderr capture), 9 (xtrace).
        assert!(w.xtrace_fds.contains("7>&2"));
        assert!(w.xtrace_fds.contains("8> >("));
        assert!(w.xtrace_fds.contains("9> >("));
    }

    #[test]
    fn other_xtrace_wrap_skips_xtrace_entirely() {
        let w = Shell::Other.xtrace_wrap("STDERR_SINK", "XTRACE_SINK");
        assert!(!w.prefix.contains("set -x"));
        assert!(!w.prefix.contains("BASH_XTRACEFD"));
        // Stderr keeps the v0.1 form (tee + passthrough).
        assert!(w.stderr_redirect.contains("2> >(tee"));
        // No xtrace FD redirect.
        assert!(w.xtrace_fds.is_empty());
    }
}
