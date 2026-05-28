use super::scanner::code_spans;
use super::shell::{detect, Shell};

/// Build the rewritten command for a given agent_key + tool_use_id.
/// Returns None if the shell doesn't support the constructs we need (safe fallback: passthrough).
pub fn rewrite(command: &str, agent_key: &str, tool_use_id: &str) -> Option<String> {
    let shell = detect();
    if !shell.supports_process_substitution() {
        return None;
    }
    rewrite_with_shell(command, agent_key, tool_use_id, shell)
}

pub(crate) fn rewrite_with_shell(
    command: &str,
    agent_key: &str,
    tool_use_id: &str,
    shell: Shell,
) -> Option<String> {
    // No proc-sub gate here — production already gated; tests can force any
    // shell variant to verify the wrap shape regardless of the running shell.
    let neutralized = neutralize_dev_null_stderr(command, agent_key, tool_use_id);
    let with_filters = inject_pre_filter_tees(&neutralized, agent_key, tool_use_id);
    let inner = inject_redirect_tees(&with_filters, agent_key, tool_use_id);
    let stdout_sink = obi_tee_invocation(agent_key, tool_use_id, "stdout");
    let stderr_sink = obi_tee_invocation(agent_key, tool_use_id, "stderr");
    let xtrace_sink = obi_tee_invocation(agent_key, tool_use_id, "xtrace");
    let wrap = shell.xtrace_wrap(&stderr_sink, &xtrace_sink);
    // Newline (not `;`) before the closing `}` so that a trailing
    // `# comment` in the inner command is terminated. With `;`, an
    // unterminated `#`-to-EOL comment swallowed the closing brace and
    // produced a zsh/bash parse error.
    Some(format!(
        "{{ {prefix}{inner}\n}} \
         > >(tee >({stdout_sink} >/dev/null)) \
         {stderr_redirect} \
         {xtrace_fds}",
        prefix = wrap.prefix,
        stderr_redirect = wrap.stderr_redirect,
        xtrace_fds = wrap.xtrace_fds,
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

/// For every bare `> FILE` or `>> FILE` redirect, replace with
/// `> >(tee [-a] FILE >(oby-tee --stream stdout-to-file …) >/dev/null)`.
/// The tee preserves the original write semantics — truncate for `>`,
/// append for `>>` — and the oby-tee capture runs in parallel via process
/// substitution. The trailing `>/dev/null` on tee suppresses tee's own
/// stdout, which otherwise would leak back as cmd's stdout.
fn inject_redirect_tees(command: &str, agent_key: &str, tool_use_id: &str) -> String {
    use super::scanner::{redirects, RedirectKind};
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for r in redirects(command) {
        let (tee_flag, stream) = match r.kind {
            RedirectKind::Out => ("", "stdout-to-file"),
            RedirectKind::Append => ("-a ", "stdout-appended-file"),
        };
        let target = &command[r.target_start..r.target_end];
        let replacement = format!(
            "> >(tee {tee_flag}{target} >({sink} >/dev/null) >/dev/null)",
            sink = obi_tee_invocation(agent_key, tool_use_id, stream),
        );
        edits.push((r.op_start, r.target_end, replacement));
    }
    apply_replacements(command, edits)
}

/// Replace each `command[start..end]` slice with the given text. Edits are
/// applied right-to-left so positions don't shift.
fn apply_replacements(command: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = command.to_string();
    for (start, end, text) in edits {
        out.replace_range(start..end, &text);
    }
    out
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
    use super::super::shell::Shell;
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
        let out = rewrite_with_shell("ls -la", "main", "t1", Shell::Bash).unwrap();
        // Wrap now leads with BASH_XTRACEFD=9 then set -x then the inner cmd.
        assert!(
            out.starts_with("{ BASH_XTRACEFD=9\nset -x\nls -la\n} "),
            "outer wrap header changed; got: {out}"
        );
        assert!(out.contains("--agent 'main'"));
        assert!(out.contains("--tool-use-id 't1'"));
        assert!(out.contains("--stream 'stdout'"));
        assert!(out.contains("--stream 'stderr'"));
        assert!(out.contains("--stream 'xtrace'"));
    }

    #[test]
    fn rewrite_includes_dev_null_neutralization() {
        let out = rewrite_with_shell("cmd 2>/dev/null", "main", "t1", Shell::Bash).unwrap();
        assert!(out.contains("--stream stderr-discarded"));
    }

    /// Regression for scenario 6 in docs/testing/v0.1-manual.md.
    /// A trailing `#` comment must NOT swallow the closing brace of the outer
    /// wrap. With `; }`, the `#` runs to the next newline and the inner block
    /// is never closed, causing a zsh parse error. With `\n}`, the newline
    /// terminates the comment.
    #[test]
    fn trailing_comment_does_not_break_outer_wrap() {
        let out = rewrite_with_shell(
            "echo ok # 2>/dev/null trailing comment",
            "main",
            "t1",
            Shell::Bash,
        )
        .unwrap();
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
        let out = rewrite_with_shell("ls | grep foo", "main", "t1", Shell::Bash).unwrap();
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
        let out =
            rewrite_with_shell("cat /etc/passwd | head -n 5", "main", "t1", Shell::Bash).unwrap();
        assert!(
            out.contains("--stream 'stdout-pre-head'"),
            "expected stdout-pre-head tee; got: {out}"
        );
        assert!(out.contains("| head -n 5"));
    }

    #[test]
    fn rewrite_tail_injects_pre_tail_tee() {
        let out =
            rewrite_with_shell("cat /etc/passwd | tail -n 3", "main", "t1", Shell::Bash).unwrap();
        assert!(out.contains("--stream 'stdout-pre-tail'"));
        assert!(out.contains("| tail -n 3"));
    }

    #[test]
    fn rewrite_first_pipe_stage_not_tee_d() {
        // The first segment is the producer; no preceding pipe to tee onto.
        // We only ever tee BEFORE a filter, not before the producer.
        let out = rewrite_with_shell("grep foo /etc/passwd", "main", "t1", Shell::Bash).unwrap();
        assert!(
            !out.contains("--stream 'stdout-pre-grep'"),
            "a bare `grep` (not piped) must not get a pre-grep tee; got: {out}"
        );
    }

    #[test]
    fn rewrite_grep_inside_quoted_string_not_rewritten() {
        let out = rewrite_with_shell(r#"echo "ls | grep foo""#, "main", "t1", Shell::Bash).unwrap();
        assert!(
            !out.contains("--stream 'stdout-pre-grep'"),
            "the quoted `| grep` is text, not a real pipe; got: {out}"
        );
    }

    #[test]
    fn rewrite_grep_in_comment_not_rewritten() {
        let out = rewrite_with_shell("echo ok # | grep foo", "main", "t1", Shell::Bash).unwrap();
        assert!(!out.contains("--stream 'stdout-pre-grep'"));
    }

    #[test]
    fn rewrite_grep_inside_subshell_not_rewritten() {
        let out = rewrite_with_shell("(cat foo | grep bar)", "main", "t1", Shell::Bash).unwrap();
        assert!(
            !out.contains("--stream 'stdout-pre-grep'"),
            "the pipe inside (...) belongs to the subshell, not the outer pipeline"
        );
    }

    #[test]
    fn rewrite_out_redirect_injects_tee_to_file() {
        let out = rewrite_with_shell("echo hi > out.txt", "main", "t1", Shell::Bash).unwrap();
        assert!(
            out.contains("> >(tee out.txt >(oby-tee --agent 'main' --tool-use-id 't1' --stream 'stdout-to-file'"),
            "expected tee + oby-tee wrap around > out.txt; got: {out}"
        );
        // Original `> out.txt` literal must NOT appear (we replaced it).
        // Tee preserves the destination, so out.txt still appears INSIDE the
        // tee call — but the literal "echo hi > out.txt " substring is gone.
        assert!(
            !out.contains("echo hi > out.txt "),
            "the unrewritten redirect should be gone; got: {out}"
        );
    }

    #[test]
    fn rewrite_append_redirect_injects_tee_a_to_file() {
        let out = rewrite_with_shell("echo more >> log.txt", "main", "t1", Shell::Bash).unwrap();
        assert!(
            out.contains("> >(tee -a log.txt >(oby-tee --agent 'main' --tool-use-id 't1' --stream 'stdout-appended-file'"),
            "expected tee -a + stdout-appended-file; got: {out}"
        );
    }

    #[test]
    fn rewrite_quoted_target_preserved() {
        let out =
            rewrite_with_shell(r#"echo hi > "out file.txt""#, "main", "t1", Shell::Bash).unwrap();
        assert!(
            out.contains(r#"tee "out file.txt""#),
            "quoted target must be preserved verbatim; got: {out}"
        );
    }

    #[test]
    fn rewrite_fd_redirect_not_touched_by_v0_2() {
        // `2> err` is FD-prefixed; v0.1's neutralize_dev_null_stderr handles
        // 2>/dev/null specifically. A non-/dev/null FD redirect is left
        // unchanged by both layers.
        let out = rewrite_with_shell("echo hi 2> err.txt", "main", "t1", Shell::Bash).unwrap();
        assert!(
            !out.contains("--stream 'stdout-to-file'"),
            "2> err.txt is not a v0.2 redirect; got: {out}"
        );
        assert!(!out.contains("--stream 'stdout-appended-file'"),);
        // The 2> err.txt substring is preserved (it falls through both passes).
        assert!(out.contains("2> err.txt"));
    }

    #[test]
    fn rewrite_dev_null_stderr_and_file_redirect_coexist() {
        // v0.1's 2>/dev/null layer fires for the 2>/dev/null, v0.2's redirect
        // layer fires for the > out.txt. Both end up in the final command.
        let out =
            rewrite_with_shell("cmd > out.txt 2>/dev/null", "main", "t1", Shell::Bash).unwrap();
        assert!(out.contains("--stream stderr-discarded"));
        assert!(out.contains("--stream 'stdout-to-file'"));
    }

    #[test]
    fn rewrite_process_substitution_target_not_re_rewritten() {
        // > >(...) is process substitution — scanner skips it.
        let out = rewrite_with_shell("cmd > >(some-cmd)", "main", "t1", Shell::Bash).unwrap();
        assert!(
            !out.contains("--stream 'stdout-to-file'"),
            "process substitution target must not be wrapped"
        );
    }

    #[test]
    fn rewrite_outer_wrap_includes_xtrace_fd() {
        let out = rewrite_with_shell("echo hi", "main", "t1", Shell::Bash).unwrap();
        // Three things must be in the wrapped command:
        // 1. set -x to enable command tracing.
        assert!(out.contains("set -x"), "set -x missing; got: {out}");
        // 2. BASH_XTRACEFD=9 to route trace to FD 9.
        assert!(
            out.contains("BASH_XTRACEFD=9"),
            "BASH_XTRACEFD=9 missing; got: {out}"
        );
        // 3. A 9> >(oby-tee --stream 'xtrace' ...) redirect on the block.
        assert!(
            out.contains("9> >(oby-tee --agent 'main' --tool-use-id 't1' --stream 'xtrace'"),
            "FD 9 → xtrace sink missing; got: {out}"
        );
    }

    #[test]
    fn rewrite_outer_wrap_preserves_stdout_and_stderr_sinks() {
        // Adding xtrace must not break the v0.1 stdout/stderr capture.
        let out = rewrite_with_shell("echo hi", "main", "t1", Shell::Bash).unwrap();
        assert!(out.contains("--stream 'stdout'"));
        assert!(out.contains("--stream 'stderr'"));
    }

    #[test]
    fn rewrite_ampersand_combined_redirect_not_touched() {
        // `&>` is deferred to v0.3+; the rewriter must leave it alone.
        let out = rewrite_with_shell("echo hi &> log.txt", "main", "t1", Shell::Bash).unwrap();
        assert!(
            out.contains("echo hi &> log.txt"),
            "&> must pass through; got: {out}"
        );
        assert!(
            !out.contains("--stream 'stdout-to-file'"),
            "&> must not be wrapped; got: {out}"
        );
    }

    #[test]
    fn rewrite_fd_duplication_not_touched() {
        // `>&2` is fd duplication; rewriting it would orphan `&2` in the
        // output and crash the shell. The rewriter must leave it alone.
        let out = rewrite_with_shell("echo error >&2", "main", "t1", Shell::Bash).unwrap();
        assert!(
            out.contains("echo error >&2"),
            ">&2 must pass through; got: {out}"
        );
        assert!(
            !out.contains("--stream 'stdout-to-file'"),
            ">&2 must not be wrapped; got: {out}"
        );
    }

    #[test]
    fn zsh_outer_wrap_includes_ps4_sentinel_and_awk_demuxer() {
        let out = rewrite_with_shell("echo hi", "main", "t1", Shell::Zsh).unwrap();
        assert!(
            out.contains(r"PS4=$'\x01"),
            "PS4 sentinel missing; got: {out}"
        );
        assert!(out.contains("set -x"), "set -x missing; got: {out}");
        assert!(out.contains("awk"), "awk demuxer missing; got: {out}");
        assert!(
            out.contains("9> >("),
            "FD 9 xtrace sink missing; got: {out}"
        );
        assert!(
            out.contains("8> >("),
            "FD 8 stderr capture missing; got: {out}"
        );
        assert!(
            out.contains("7>&2"),
            "FD 7 stderr passthrough missing; got: {out}"
        );
    }

    #[test]
    fn zsh_wrap_does_not_use_bash_xtracefd() {
        let out = rewrite_with_shell("echo hi", "main", "t1", Shell::Zsh).unwrap();
        assert!(
            !out.contains("BASH_XTRACEFD"),
            "BASH_XTRACEFD must not appear in zsh wrap; got: {out}"
        );
    }

    #[test]
    fn other_shell_skips_xtrace() {
        let out = rewrite_with_shell("echo hi", "main", "t1", Shell::Other).unwrap();
        assert!(!out.contains("set -x"));
        assert!(!out.contains("BASH_XTRACEFD"));
        assert!(!out.contains("PS4="));
        // No FD 9 redirect either.
        assert!(!out.contains("9> >("));
        // But stdout/stderr capture still works (v0.1 baseline).
        assert!(out.contains("--stream 'stdout'"));
        assert!(out.contains("--stream 'stderr'"));
    }
}
