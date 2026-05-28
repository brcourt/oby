use oby_core::{Capturer, DisplayEntry, DisplayEntryUpdate, EntryBody, EntryStatus, HookContext};
use serde::Deserialize;
use serde_json::Value;
use std::time::SystemTime;

#[allow(dead_code)]
pub struct ReadCapturer;

#[allow(dead_code)]
#[derive(Deserialize)]
struct ReadInput {
    file_path: String,
    #[serde(default)]
    offset: Option<u32>,
    #[serde(default)]
    limit: Option<u32>,
}

impl Capturer for ReadCapturer {
    fn name(&self) -> &'static str {
        "read"
    }

    fn tool_name(&self) -> &'static str {
        "Read"
    }

    fn render_pre(&self, ctx: &HookContext, input: &Value) -> Option<DisplayEntry> {
        let input: ReadInput = serde_json::from_value(input.clone()).ok()?;
        let mut headline = format!("Read  {}", input.file_path);
        if let (Some(o), Some(l)) = (input.offset, input.limit) {
            headline.push_str(&format!("  [{}..{}]", o, o + l));
        }
        Some(DisplayEntry {
            agent_key: ctx.agent_key().to_string(),
            tool_use_id: ctx.tool_use_id.clone(),
            tool: "read".to_string(),
            timestamp: SystemTime::now(),
            headline,
            body: EntryBody::None,
            status: EntryStatus::Pending,
        })
    }

    fn render_post(
        &self,
        ctx: &HookContext,
        _input: &Value,
        response: &Value,
    ) -> Option<DisplayEntryUpdate> {
        // Best-effort: CC's Read response is structured but field names vary.
        // For v0.1 we just count bytes if a stringy body is present, else mark Ok.
        let text_len = response
            .get("output")
            .and_then(|v| v.as_str())
            .map(|s| s.len())
            .unwrap_or(0);
        let summary = if text_len > 0 {
            format!("{} bytes", text_len)
        } else {
            "ok".to_string()
        };
        Some(DisplayEntryUpdate {
            tool_use_id: ctx.tool_use_id.clone(),
            status: EntryStatus::Ok,
            append_body: Some(EntryBody::Text { text: summary }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(tool: &str) -> HookContext {
        serde_json::from_str(&format!(
            r#"{{"session_id":"s","transcript_path":"/t","cwd":"/c","hook_event_name":"PreToolUse","tool_name":"{tool}","tool_use_id":"t1"}}"#
        ))
        .unwrap()
    }

    #[test]
    fn render_pre_emits_headline() {
        let c = ReadCapturer;
        let ctx = ctx("Read");
        let input = serde_json::json!({"file_path": "/a/b/foo.ts"});
        let entry = c.render_pre(&ctx, &input).unwrap();
        assert_eq!(entry.headline, "Read  /a/b/foo.ts");
        assert_eq!(entry.tool, "read");
        assert_eq!(entry.status, EntryStatus::Pending);
    }

    #[test]
    fn render_pre_with_offset_and_limit() {
        let c = ReadCapturer;
        let ctx = ctx("Read");
        let input = serde_json::json!({"file_path": "/a/b/foo.ts", "offset": 100, "limit": 50});
        let entry = c.render_pre(&ctx, &input).unwrap();
        assert_eq!(entry.headline, "Read  /a/b/foo.ts  [100..150]");
    }

    #[test]
    fn render_post_uses_ctx_tool_use_id_and_byte_count() {
        let c = ReadCapturer;
        let ctx = ctx("Read");
        let input = serde_json::json!({"file_path": "/a/b/foo.ts"});
        let response = serde_json::json!({"output": "hello world\nbye"});
        let upd = c.render_post(&ctx, &input, &response).unwrap();
        assert_eq!(upd.tool_use_id, "t1"); // from ctx, not from response
        assert_eq!(upd.status, EntryStatus::Ok);
        match upd.append_body {
            Some(EntryBody::Text { text }) => assert!(text.contains("bytes")),
            _ => panic!("expected Text body"),
        }
    }

    #[test]
    fn render_post_falls_back_to_ok_summary_when_no_output() {
        let c = ReadCapturer;
        let ctx = ctx("Read");
        let input = serde_json::json!({"file_path": "/a/b/foo.ts"});
        let response = serde_json::json!({});
        let upd = c.render_post(&ctx, &input, &response).unwrap();
        assert_eq!(upd.tool_use_id, "t1");
        match upd.append_body {
            Some(EntryBody::Text { text }) => assert_eq!(text, "ok"),
            _ => panic!("expected Text body"),
        }
    }
}
