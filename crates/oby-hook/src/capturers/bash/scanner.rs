/// A span of the input that is NOT inside a quoted string or after a comment.
/// (start, end) are byte indices into the original string.
#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
pub struct CodeSpan {
    pub start: usize,
    pub end: usize,
}

/// Walk a shell command and return spans of unquoted-and-non-commented code.
/// Recognizes: '…' (no escapes), "…" (with \ escapes), # comment (to EOL).
/// `$(…)` and backticks are treated as code (we recurse no further).
#[allow(dead_code)]
pub fn code_spans(s: &str) -> Vec<CodeSpan> {
    let bytes = s.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    let mut span_start = 0usize;

    macro_rules! flush_to {
        ($end:expr) => {
            if span_start < $end {
                spans.push(CodeSpan {
                    start: span_start,
                    end: $end,
                });
            }
        };
    }

    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'\'' => {
                flush_to!(i);
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
                span_start = i;
            }
            b'"' => {
                flush_to!(i);
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' if i + 1 < bytes.len() => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                span_start = i;
            }
            b'#' => {
                let prev_is_ws =
                    i == 0 || matches!(bytes[i - 1], b' ' | b'\t' | b'\n' | b';' | b'|' | b'&');
                if prev_is_ws {
                    flush_to!(i);
                    while i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                    span_start = i;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    flush_to!(bytes.len());
    spans
}

/// Convenience: does any code span (unquoted region) contain the given substring?
#[allow(dead_code)]
pub fn code_contains(s: &str, needle: &str) -> bool {
    for span in code_spans(s) {
        if s[span.start..span.end].contains(needle) {
            return true;
        }
    }
    false
}

/// A pipe-separated segment of a shell pipeline. `start..end` is the byte
/// range of the segment's text (exclusive of the surrounding `|` characters).
/// `first_word_start..first_word_end` points at the first whitespace-delimited
/// token in the segment — typically the command name (`grep`, `head`, …).
#[derive(Debug, PartialEq, Eq)]
pub struct PipeSegment {
    pub start: usize,
    pub end: usize,
    pub first_word_start: usize,
    pub first_word_end: usize,
}

/// Walk the input shell command and emit one `PipeSegment` per pipe stage.
/// A two-stage pipeline `cmd1 | cmd2` produces two segments; a single
/// command produces one. Pipes inside quotes, comments, parentheses,
/// `$( … )` command substitution, backticks, or `[[ … ]]` test expressions
/// are excluded — they're part of the surrounding segment, not separators.
/// The logical-or operator `||` is NOT treated as a pipe.
pub fn pipe_segments(s: &str) -> Vec<PipeSegment> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut seg_start = 0usize;

    // Nesting depth counters. We treat a non-zero depth as "do not emit
    // a pipe even if you see a `|`."
    let mut paren_depth: u32 = 0; // ( ... ) and $( ... )
    let mut bracket_depth: u32 = 0; // [[ ... ]]
    let mut backtick_depth: u32 = 0; // ` ... `

    let flush = |out: &mut Vec<PipeSegment>, s: &str, start: usize, end: usize| {
        let (fw_start, fw_end) = first_word(s, start, end);
        out.push(PipeSegment {
            start,
            end,
            first_word_start: fw_start,
            first_word_end: fw_end,
        });
    };

    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'\'' => {
                // Single-quoted string: no escapes, just find the next `'`.
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
            }
            b'"' => {
                // Double-quoted string: support `\` escapes.
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' if i + 1 < bytes.len() => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'#' => {
                // Comment-to-EOL only when preceded by whitespace or start-of-cmd
                // or a shell separator (matches code_spans behavior).
                let prev_is_ws =
                    i == 0 || matches!(bytes[i - 1], b' ' | b'\t' | b'\n' | b';' | b'|' | b'&');
                if prev_is_ws {
                    while i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'(' => {
                // Command substitution `$( ... )`. Open a paren depth.
                paren_depth += 1;
                i += 2;
            }
            b'(' => {
                paren_depth += 1;
                i += 1;
            }
            b')' => {
                paren_depth = paren_depth.saturating_sub(1);
                i += 1;
            }
            b'[' if i + 1 < bytes.len() && bytes[i + 1] == b'[' => {
                bracket_depth += 1;
                i += 2;
            }
            b']' if i + 1 < bytes.len() && bytes[i + 1] == b']' => {
                bracket_depth = bracket_depth.saturating_sub(1);
                i += 2;
            }
            b'`' => {
                if backtick_depth == 0 {
                    backtick_depth = 1;
                } else {
                    backtick_depth = 0;
                }
                i += 1;
            }
            b'|' if paren_depth == 0
                && bracket_depth == 0
                && backtick_depth == 0
                && i + 1 < bytes.len()
                && bytes[i + 1] != b'|' =>
            {
                // Real pipe separator. Emit the segment ending at this `|`.
                flush(&mut out, s, seg_start, i);
                i += 1;
                // Skip whitespace so seg_start lands on the next command.
                while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                }
                seg_start = i;
            }
            b'|' => {
                // Either `||` (logical or, skip both bytes) or inside a nesting
                // context. Either way, advance past this byte.
                if i + 1 < bytes.len() && bytes[i + 1] == b'|' {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    // Flush the final segment.
    flush(&mut out, s, seg_start, bytes.len());
    out
}

/// File-redirect operator and target.
#[derive(Debug, PartialEq, Eq)]
pub struct Redirect {
    pub kind: RedirectKind,
    /// Byte offset of the `>` (or first `>` in `>>`).
    pub op_start: usize,
    /// Byte offset just past the operator (`>` is 1 byte; `>>` is 2).
    pub op_end: usize,
    /// Byte range of the redirect target (file path), possibly quoted.
    pub target_start: usize,
    pub target_end: usize,
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum RedirectKind {
    /// `> FILE`
    Out,
    /// `>> FILE`
    Append,
}

/// Find bare `>` and `>>` redirects in `s`. FD-prefixed redirects (`2>`,
/// `2>>`, `&>`, etc.) and process substitution (`>(...)`) are intentionally
/// excluded — those are either handled by v0.1's existing 2>/dev/null
/// rewrite path or out of scope for v0.2.
pub fn redirects(s: &str) -> Vec<Redirect> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;

    let mut paren_depth: u32 = 0;
    let mut bracket_depth: u32 = 0;
    let mut backtick_depth: u32 = 0;

    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'\'' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' if i + 1 < bytes.len() => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'#' => {
                let prev_is_ws =
                    i == 0 || matches!(bytes[i - 1], b' ' | b'\t' | b'\n' | b';' | b'|' | b'&');
                if prev_is_ws {
                    while i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'(' => {
                paren_depth += 1;
                i += 2;
            }
            b'(' => {
                paren_depth += 1;
                i += 1;
            }
            b')' => {
                paren_depth = paren_depth.saturating_sub(1);
                i += 1;
            }
            b'[' if i + 1 < bytes.len() && bytes[i + 1] == b'[' => {
                bracket_depth += 1;
                i += 2;
            }
            b']' if i + 1 < bytes.len() && bytes[i + 1] == b']' => {
                bracket_depth = bracket_depth.saturating_sub(1);
                i += 2;
            }
            b'`' => {
                if backtick_depth == 0 {
                    backtick_depth = 1;
                } else {
                    backtick_depth = 0;
                }
                i += 1;
            }
            b'>' if paren_depth == 0 && bracket_depth == 0 && backtick_depth == 0 => {
                // Reject FD-prefixed redirects: scan one byte left for a digit.
                let preceded_by_digit = i > 0 && bytes[i - 1].is_ascii_digit();
                if preceded_by_digit {
                    i += 1;
                    continue;
                }
                let is_append = i + 1 < bytes.len() && bytes[i + 1] == b'>';
                let op_start = i;
                let op_end = if is_append { i + 2 } else { i + 1 };
                // Reject `> >(...)` process substitution: peek past whitespace
                // for an unquoted `(`.
                let mut probe = op_end;
                while probe < bytes.len() && (bytes[probe] == b' ' || bytes[probe] == b'\t') {
                    probe += 1;
                }
                // Detect both `>(...)` (process substitution used directly as
                // target) and the literal `>` at probe being followed by `(`.
                let is_proc_subst = probe < bytes.len()
                    && (bytes[probe] == b'('
                        || (bytes[probe] == b'>'
                            && probe + 1 < bytes.len()
                            && bytes[probe + 1] == b'('));
                if is_proc_subst {
                    // process substitution; skip this redirect entirely.
                    i = op_end;
                    continue;
                }
                // Parse target word.
                let (target_start, target_end) = parse_redirect_target(bytes, op_end);
                out.push(Redirect {
                    kind: if is_append {
                        RedirectKind::Append
                    } else {
                        RedirectKind::Out
                    },
                    op_start,
                    op_end,
                    target_start,
                    target_end,
                });
                i = target_end;
            }
            _ => i += 1,
        }
    }
    out
}

/// Find the next shell word starting at `from`. Quoted words are returned
/// with their surrounding quotes intact. Stops at whitespace, `;`, `&`, `|`,
/// or EOL.
fn parse_redirect_target(bytes: &[u8], from: usize) -> (usize, usize) {
    let mut i = from;
    // Skip leading whitespace.
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    let start = i;
    if i >= bytes.len() {
        return (start, start);
    }
    match bytes[i] {
        b'\'' => {
            i += 1;
            while i < bytes.len() && bytes[i] != b'\'' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1; // consume closing quote
            }
        }
        b'"' => {
            i += 1;
            while i < bytes.len() {
                match bytes[i] {
                    b'\\' if i + 1 < bytes.len() => i += 2,
                    b'"' => {
                        i += 1;
                        break;
                    }
                    _ => i += 1,
                }
            }
        }
        _ => {
            while i < bytes.len()
                && bytes[i] != b' '
                && bytes[i] != b'\t'
                && bytes[i] != b'\n'
                && bytes[i] != b';'
                && bytes[i] != b'&'
                && bytes[i] != b'|'
            {
                i += 1;
            }
        }
    }
    (start, i)
}

/// Find the first whitespace-delimited word in `s[start..end]`. Returns
/// (word_start, word_end) — byte offsets into `s`. If the slice is all
/// whitespace, returns (end, end).
fn first_word(s: &str, start: usize, end: usize) -> (usize, usize) {
    let bytes = s.as_bytes();
    let mut i = start;
    while i < end && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n') {
        i += 1;
    }
    let word_start = i;
    while i < end && bytes[i] != b' ' && bytes[i] != b'\t' && bytes[i] != b'\n' && bytes[i] != b';'
    {
        i += 1;
    }
    (word_start, i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans_text(s: &str) -> Vec<&str> {
        code_spans(s)
            .into_iter()
            .map(|sp| &s[sp.start..sp.end])
            .collect()
    }

    #[test]
    fn no_quotes_one_span() {
        assert_eq!(spans_text("ls -la /tmp"), vec!["ls -la /tmp"]);
    }

    #[test]
    fn single_quotes_excluded() {
        assert_eq!(spans_text("echo 'hi 2>/dev/null there'"), vec!["echo "]);
    }

    #[test]
    fn double_quotes_excluded() {
        assert_eq!(
            spans_text(r#"echo "in here 2>/dev/null" out"#),
            vec!["echo ", " out"]
        );
    }

    #[test]
    fn comment_excluded_after_whitespace() {
        assert_eq!(spans_text("ls # but not 2>/dev/null"), vec!["ls "]);
    }

    #[test]
    fn hash_in_word_is_kept() {
        assert_eq!(spans_text("echo a#b"), vec!["echo a#b"]);
    }

    #[test]
    fn code_contains_ignores_quoted_matches() {
        assert!(!code_contains("echo 'foo 2>/dev/null bar'", "2>/dev/null"));
        assert!(code_contains("ls 2>/dev/null", "2>/dev/null"));
    }

    fn first_words(s: &str) -> Vec<&str> {
        pipe_segments(s)
            .into_iter()
            .map(|seg| &s[seg.first_word_start..seg.first_word_end])
            .collect()
    }

    #[test]
    fn pipe_segments_single_command_one_segment() {
        let segs = pipe_segments("ls -la /tmp");
        assert_eq!(segs.len(), 1);
        assert_eq!(first_words("ls -la /tmp"), vec!["ls"]);
    }

    #[test]
    fn pipe_segments_two_stage_pipeline() {
        let segs = pipe_segments("ls | grep foo");
        assert_eq!(segs.len(), 2);
        assert_eq!(first_words("ls | grep foo"), vec!["ls", "grep"]);
    }

    #[test]
    fn pipe_segments_three_stage_pipeline() {
        assert_eq!(
            first_words("cat /etc/hosts | grep 127 | head -n 5"),
            vec!["cat", "grep", "head"]
        );
    }

    #[test]
    fn pipe_segments_logical_or_is_one_segment() {
        // `||` is logical-or, NOT a pipe. The whole thing is one segment.
        let segs = pipe_segments("ls /nope || echo fallback");
        assert_eq!(segs.len(), 1);
    }

    #[test]
    fn pipe_segments_pipe_in_single_quote_excluded() {
        let segs = pipe_segments("echo 'a | b' | grep foo");
        // The `|` inside '...' is text, not a pipe; the second `|` is real.
        assert_eq!(segs.len(), 2);
        assert_eq!(first_words("echo 'a | b' | grep foo"), vec!["echo", "grep"]);
    }

    #[test]
    fn pipe_segments_pipe_in_double_quote_excluded() {
        let segs = pipe_segments(r#"echo "a | b" | grep foo"#);
        assert_eq!(segs.len(), 2);
    }

    #[test]
    fn pipe_segments_pipe_in_comment_excluded() {
        let segs = pipe_segments("echo ok # a | b");
        assert_eq!(segs.len(), 1);
    }

    #[test]
    fn pipe_segments_pipe_in_parens_excluded() {
        // The pipe inside (...) is a subshell pipe; outer scanner sees one segment.
        let segs = pipe_segments("(echo a | grep b)");
        assert_eq!(segs.len(), 1);
    }

    #[test]
    fn pipe_segments_pipe_in_command_substitution_excluded() {
        let segs = pipe_segments("echo $(cat foo | wc -l)");
        assert_eq!(segs.len(), 1);
    }

    #[test]
    fn pipe_segments_pipe_in_backticks_excluded() {
        let segs = pipe_segments("echo `cat foo | wc -l`");
        assert_eq!(segs.len(), 1);
    }

    #[test]
    fn pipe_segments_fd_prefixed_pipe_still_pipe() {
        // `2>` is a redirect operator, but `|` between commands is always a pipe
        // regardless of any prefix. This is a sanity check.
        assert_eq!(first_words("cmd 2>err | grep foo"), vec!["cmd", "grep"]);
    }

    #[test]
    fn pipe_segments_first_word_skips_leading_whitespace() {
        let segs = pipe_segments("ls |    grep foo");
        assert_eq!(segs.len(), 2);
        let s = "ls |    grep foo";
        // Second segment's first word starts at the 'g' of 'grep', not at the space.
        assert_eq!(&s[segs[1].first_word_start..segs[1].first_word_end], "grep");
    }

    #[test]
    fn redirects_simple_out() {
        let r = redirects("ls > out.txt");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].kind, RedirectKind::Out);
        let s = "ls > out.txt";
        assert_eq!(&s[r[0].target_start..r[0].target_end], "out.txt");
    }

    #[test]
    fn redirects_simple_append() {
        let r = redirects("ls >> out.txt");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].kind, RedirectKind::Append);
        let s = "ls >> out.txt";
        assert_eq!(&s[r[0].target_start..r[0].target_end], "out.txt");
    }

    #[test]
    fn redirects_quoted_target_double() {
        let r = redirects(r#"ls > "foo bar.txt""#);
        assert_eq!(r.len(), 1);
        let s = r#"ls > "foo bar.txt""#;
        assert_eq!(&s[r[0].target_start..r[0].target_end], r#""foo bar.txt""#);
    }

    #[test]
    fn redirects_quoted_target_single() {
        let r = redirects("ls > 'a b c'");
        assert_eq!(r.len(), 1);
        let s = "ls > 'a b c'";
        assert_eq!(&s[r[0].target_start..r[0].target_end], "'a b c'");
    }

    #[test]
    fn redirects_fd_prefixed_redirect_excluded() {
        // `2> err` is FD-specific (v0.1 handles 2>/dev/null already). The
        // v0.2 scanner emits nothing for digit-prefixed redirects.
        let r = redirects("ls 2> err.txt");
        assert!(r.is_empty());
    }

    #[test]
    fn redirects_process_substitution_excluded() {
        // `> >(...)` is a redirect to a process substitution, not a file.
        // We must not match this as a file redirect.
        let r = redirects("ls > >(tee /tmp/x >/dev/null)");
        assert!(r.is_empty());
    }

    #[test]
    fn redirects_quoted_metachar_excluded() {
        // `>` inside quotes is text, not a redirect.
        let r = redirects(r#"echo "a > b""#);
        assert!(r.is_empty());
    }

    #[test]
    fn redirects_comment_excluded() {
        let r = redirects("ls # > out.txt");
        assert!(r.is_empty());
    }

    #[test]
    fn redirects_multiple() {
        // `cmd > a >> b` is unusual but legal: stdout to a, then reopened
        // appending to b. Both should be emitted.
        let r = redirects("cmd > a >> b");
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].kind, RedirectKind::Out);
        assert_eq!(r[1].kind, RedirectKind::Append);
    }

    #[test]
    fn redirects_inside_command_substitution_excluded() {
        let r = redirects("echo $(cat > /tmp/x)");
        assert!(r.is_empty());
    }
}
