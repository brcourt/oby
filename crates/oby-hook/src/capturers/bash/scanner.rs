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
}
