use super::scanner::code_spans;
use super::shell::detect;

/// Build the rewritten command for a given agent_key + tool_use_id.
/// Returns None if the shell doesn't support the constructs we need (safe fallback: passthrough).
pub fn rewrite(command: &str, agent_key: &str, tool_use_id: &str) -> Option<String> {
    if !detect().supports_process_substitution() {
        return None;
    }
    let neutralized = neutralize_dev_null_stderr(command, agent_key, tool_use_id);
    let inner = inject_pre_filter_tees(&neutralized, agent_key, tool_use_id);
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

/// For every pipe segment whose first word is a "discarding filter"
/// (grep / head / tail), insert a `tee >(oby-tee --stream stdout-pre-<filter>)
/// |` immediately before that segment. The original filter still consumes
/// the producer's output; oby-tee gets a parallel copy via process
/// substitution.
///
/// Multi-stage pipelines like `cmd | grep | head` get a tee at EACH matching
/// filter; the chunks land as `stdout-pre-grep` and `stdout-pre-head`.
fn inject_pre_filter_tees(command: &str, agent_key: &str, tool_use_id: &str) -> String {
    use super::scanner::pipe_segments;
    const FILTERS: &[(&str, &str)] = &[
        ("grep", "stdout-pre-grep"),
        ("head", "stdout-pre-head"),
        ("tail", "stdout-pre-tail"),
    ];

    let segments = pipe_segments(command);
    // Collect (insert_at, text) edits; apply right-to-left so positions
    // don't shift.
    let mut edits: Vec<(usize, String)> = Vec::new();
    for (idx, seg) in segments.iter().enumerate() {
        if idx == 0 {
            // First segment is the producer — no preceding `|` to tee onto.
            continue;
        }
        let first_word = &command[seg.first_word_start..seg.first_word_end];
        let Some((_, stream)) = FILTERS.iter().find(|(name, _)| *name == first_word) else {
            continue;
        };
        let tee = format!(
            "tee >({} >/dev/null) | ",
            obi_tee_invocation(agent_key, tool_use_id, stream),
        );
        edits.push((seg.start, tee));
    }
    apply_inserts(command, edits)
}

/// Insert `text` at each given byte offset in `command`, processing edits
/// from right to left so earlier inserts don't shift later positions.
fn apply_inserts(command: &str, mut edits: Vec<(usize, String)>) -> String {
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = command.to_string();
    for (pos, text) in edits {
        out.insert_str(pos, &text);
    }
    out
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

    #[test]
    fn rewrite_grep_injects_pre_grep_tee() {
        let out = rewrite("ls | grep foo", "main", "t1").unwrap();
        assert!(
            out.contains(
                "tee >(oby-tee --agent 'main' --tool-use-id 't1' --stream 'stdout-pre-grep'"
            ),
            "expected stdout-pre-grep tee; got: {out}"
        );
        // The original | grep stage must still be there.
        assert!(
            out.contains("| grep foo"),
            "original | grep must be preserved; got: {out}"
        );
    }

    #[test]
    fn rewrite_head_injects_pre_head_tee() {
        let out = rewrite("cat /etc/passwd | head -n 5", "main", "t1").unwrap();
        assert!(
            out.contains("--stream 'stdout-pre-head'"),
            "expected stdout-pre-head tee; got: {out}"
        );
        assert!(out.contains("| head -n 5"));
    }

    #[test]
    fn rewrite_tail_injects_pre_tail_tee() {
        let out = rewrite("cat /etc/passwd | tail -n 3", "main", "t1").unwrap();
        assert!(out.contains("--stream 'stdout-pre-tail'"));
        assert!(out.contains("| tail -n 3"));
    }

    #[test]
    fn rewrite_first_pipe_stage_not_tee_d() {
        // The first segment is the producer; no preceding pipe to tee onto.
        // We only ever tee BEFORE a filter, not before the producer.
        let out = rewrite("grep foo /etc/passwd", "main", "t1").unwrap();
        assert!(
            !out.contains("--stream 'stdout-pre-grep'"),
            "a bare `grep` (not piped) must not get a pre-grep tee; got: {out}"
        );
    }

    #[test]
    fn rewrite_grep_inside_quoted_string_not_rewritten() {
        let out = rewrite(r#"echo "ls | grep foo""#, "main", "t1").unwrap();
        assert!(
            !out.contains("--stream 'stdout-pre-grep'"),
            "the quoted `| grep` is text, not a real pipe; got: {out}"
        );
    }

    #[test]
    fn rewrite_grep_in_comment_not_rewritten() {
        let out = rewrite("echo ok # | grep foo", "main", "t1").unwrap();
        assert!(!out.contains("--stream 'stdout-pre-grep'"));
    }

    #[test]
    fn rewrite_grep_inside_subshell_not_rewritten() {
        let out = rewrite("(cat foo | grep bar)", "main", "t1").unwrap();
        assert!(
            !out.contains("--stream 'stdout-pre-grep'"),
            "the pipe inside (...) belongs to the subshell, not the outer pipeline"
        );
    }
}
