use super::scanner::code_spans;
use super::shell::detect;

/// Build the rewritten command for a given agent_key + tool_use_id.
/// Returns None if the shell doesn't support the constructs we need (safe fallback: passthrough).
pub fn rewrite(command: &str, agent_key: &str, tool_use_id: &str) -> Option<String> {
    if !detect().supports_process_substitution() {
        return None;
    }
    let inner = neutralize_dev_null_stderr(command, agent_key, tool_use_id);
    let stdout_sink = obi_tee_invocation(agent_key, tool_use_id, "stdout");
    let stderr_sink = obi_tee_invocation(agent_key, tool_use_id, "stderr");
    // Newline (not `;`) before the closing `}` so that a trailing
    // `# comment` in the inner command is terminated. With `;`, an
    // unterminated `#`-to-EOL comment swallowed the closing brace and
    // produced a zsh/bash parse error.
    Some(format!(
        "{{ {inner}\n}} > >(tee >({stdout_sink} >/dev/null)) 2> >(tee >({stderr_sink} >/dev/null) >&2)"
    ))
}

fn obi_tee_invocation(agent_key: &str, tool_use_id: &str, stream: &str) -> String {
    format!(
        "oby-tee --agent {} --tool-use-id {} --stream {}",
        shell_escape(agent_key),
        shell_escape(tool_use_id),
        shell_escape(stream),
    )
}

/// Replace each unquoted `2>/dev/null` with `2> >(oby-tee … --stream stderr-discarded)`.
fn neutralize_dev_null_stderr(command: &str, agent_key: &str, tool_use_id: &str) -> String {
    let needle = "2>/dev/null";
    let sink = format!(
        ">(oby-tee --agent {} --tool-use-id {} --stream stderr-discarded >/dev/null)",
        shell_escape(agent_key),
        shell_escape(tool_use_id),
    );
    let spans = code_spans(command);
    let mut out = String::with_capacity(command.len() + 64);
    let mut cursor = 0;
    for span in spans {
        if cursor < span.start {
            out.push_str(&command[cursor..span.start]);
        }
        let region = &command[span.start..span.end];
        let mut local = 0;
        while let Some(pos) = region[local..].find(needle) {
            let abs = local + pos;
            out.push_str(&region[local..abs]);
            out.push_str("2> ");
            out.push_str(&sink);
            local = abs + needle.len();
        }
        out.push_str(&region[local..]);
        cursor = span.end;
    }
    if cursor < command.len() {
        out.push_str(&command[cursor..]);
    }
    out
}

/// Minimal shell-safe escape for arguments we control (agent_id, tool_use_id, stream names).
/// Returns the string wrapped in single quotes with any internal `'` replaced by `'\''`.
fn shell_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_escape_basic() {
        assert_eq!(shell_escape("main"), "'main'");
        assert_eq!(shell_escape("a'b"), "'a'\\''b'");
    }

    #[test]
    fn dev_null_inside_quotes_is_not_rewritten() {
        let cmd = "echo 'leave 2>/dev/null alone' && ls 2>/dev/null";
        let out = neutralize_dev_null_stderr(cmd, "main", "t1");
        assert!(
            out.contains("'leave 2>/dev/null alone'"),
            "quoted region must be preserved"
        );
        assert!(
            out.contains("2> >(oby-tee"),
            "unquoted 2>/dev/null must be rewritten"
        );
    }

    #[test]
    fn outer_wrap_form() {
        // We rely on the test env running bash/zsh — should hold on the dev machine.
        let out = rewrite("ls -la", "main", "t1").expect("bash/zsh expected in test env");
        assert!(out.starts_with("{ ls -la\n} > >(tee >(oby-tee"));
        assert!(out.contains("--agent 'main'"));
        assert!(out.contains("--tool-use-id 't1'"));
        assert!(out.contains("--stream 'stdout'"));
        assert!(out.contains("--stream 'stderr'"));
    }

    #[test]
    fn rewrite_includes_dev_null_neutralization() {
        let out = rewrite("cmd 2>/dev/null", "main", "t1").unwrap();
        assert!(out.contains("--stream stderr-discarded"));
    }

    /// Regression for scenario 6 in docs/testing/v0.1-manual.md.
    /// A trailing `#` comment must NOT swallow the closing brace of the outer
    /// wrap. With `; }`, the `#` runs to the next newline and the inner block
    /// is never closed, causing a zsh parse error. With `\n}`, the newline
    /// terminates the comment.
    #[test]
    fn trailing_comment_does_not_break_outer_wrap() {
        let out = rewrite("echo ok # 2>/dev/null trailing comment", "main", "t1").unwrap();
        // The closing brace must be on its own line (post-comment), not on the
        // same line as the comment.
        assert!(
            out.contains("trailing comment\n}"),
            "newline must follow the inner so a trailing # comment is terminated; got: {out}"
        );
        // And the comment region must NOT have been rewritten — the 2>/dev/null
        // inside the # comment is just text, the scanner correctly excluded it.
        assert!(
            !out.contains("# 2> >(oby-tee"),
            "the 2>/dev/null inside a # comment must not be rewritten; got: {out}"
        );
    }
}
