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
                // Multi-char sentinel `__OBYXT__` prepended to zsh's native format.
                // The sentinel is printable so it survives any shell pipeline
                // intermediates; the demuxer strips it before forwarding to the
                // xtrace sink, leaving the zsh-native `+(source):line> cmd` prefix.
                //
                // awk demuxer routes each line:
                //   /^__OBYXT__/  → strip sentinel, pipe to xtrace_sink
                //   else          → pipe to stderr_sink AND print to /dev/stderr
                //                   (so claude still sees the agent's real stderr)
                //
                // Sink commands are passed via -v so the awk script itself doesn't
                // embed shell-quoted commands. Double-quote the -v values so the
                // inner single quotes (from shell_escape) survive.
                let prefix = "PS4='__OBYXT__+(%N):%i> '\nset -x\n".to_string();
                let demuxer = format!(
                    r#"awk -v xt="{xtrace_sink} >/dev/null" -v err="{stderr_sink} >/dev/null" '/^__OBYXT__/{{sub(/^__OBYXT__/, ""); print | xt; next}} {{print | err; print > "/dev/stderr"}}'"#,
                );
                XtraceWrap {
                    prefix,
                    stderr_redirect: format!("2> >({demuxer})"),
                    // Block-level FDs no longer needed for zsh — awk pipes directly
                    // to sink commands via subshells. The block keeps stdout/stderr
                    // wrappers but drops the FD 7/8/9 dance.
                    xtrace_fds: String::new(),
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
    /// `9> >(xtrace_sink)`; under zsh this is empty because the awk demuxer
    /// inside the `2> >(...)` process substitution pipes directly to sink
    /// commands via subshells. Empty string under shells that don't get xtrace.
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
        // PS4 prepends the multi-char sentinel so awk can split.
        assert!(
            w.prefix.contains("PS4='__OBYXT__"),
            "zsh prefix must set PS4 with __OBYXT__ sentinel; got: {}",
            w.prefix
        );
        assert!(w.prefix.contains("set -x"));
        // Stderr redirect contains the awk demuxer.
        assert!(
            w.stderr_redirect.contains("awk"),
            "zsh stderr redirect must use awk demuxer; got: {}",
            w.stderr_redirect
        );
        // Demuxer pipes to both sinks via subshells (`print | xt` / `print | err`),
        // and the sink commands appear as -v values.
        assert!(w.stderr_redirect.contains("XTRACE_SINK"));
        assert!(w.stderr_redirect.contains("STDERR_SINK"));
        // No block-level xtrace FDs needed — awk handles all routing.
        assert!(w.xtrace_fds.is_empty());
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
