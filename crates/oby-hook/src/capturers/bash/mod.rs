pub mod shell;

use oby_core::{
    Capturer, DisplayEntry, DisplayEntryUpdate, EntryBody, EntryStatus, HookContext,
    RewriteDecision,
};
use serde::Deserialize;
use serde_json::Value;
use std::time::SystemTime;

#[allow(dead_code)]
#[derive(Default)]
pub struct BashCapturer;

#[allow(dead_code)]
#[derive(Deserialize)]
struct BashInput {
    command: String,
    #[serde(default)]
    description: Option<String>,
}

impl Capturer for BashCapturer {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn tool_name(&self) -> &'static str {
        "Bash"
    }

    fn render_pre(&self, ctx: &HookContext, input: &Value) -> Option<DisplayEntry> {
        let input: BashInput = serde_json::from_value(input.clone()).ok()?;
        let headline = if let Some(desc) = &input.description {
            format!("Bash  {}  ({})", first_line(&input.command), desc)
        } else {
            format!("Bash  {}", first_line(&input.command))
        };
        Some(DisplayEntry {
            agent_key: ctx.agent_key().to_string(),
            tool_use_id: ctx.tool_use_id.clone(),
            tool: "bash".to_string(),
            timestamp: SystemTime::now(),
            headline,
            body: EntryBody::LiveStream {
                tool_use_id: ctx.tool_use_id.clone(),
            },
            status: EntryStatus::Pending,
        })
    }

    fn render_post(
        &self,
        ctx: &HookContext,
        _input: &Value,
        response: &Value,
    ) -> Option<DisplayEntryUpdate> {
        let status = if response.get("error").is_some() {
            EntryStatus::Error
        } else {
            EntryStatus::Ok
        };
        Some(DisplayEntryUpdate {
            tool_use_id: ctx.tool_use_id.clone(),
            status,
            append_body: None,
        })
    }

    fn pre_rewrite(&self, _ctx: &HookContext, _input: &Value) -> RewriteDecision {
        // Filled in by Epic 5.
        RewriteDecision::Passthrough
    }
}

#[allow(dead_code)]
fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> HookContext {
        serde_json::from_str(
            r#"{"session_id":"s","transcript_path":"/t","cwd":"/c","hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"t1"}"#
        ).unwrap()
    }

    #[test]
    fn render_pre_uses_first_line_of_command() {
        let c = BashCapturer;
        let input = serde_json::json!({"command": "echo hi\necho bye"});
        let entry = c.render_pre(&ctx(), &input).unwrap();
        assert_eq!(entry.headline, "Bash  echo hi");
        match entry.body {
            EntryBody::LiveStream { tool_use_id } => assert_eq!(tool_use_id, "t1"),
            _ => panic!("expected LiveStream body"),
        }
    }

    #[test]
    fn render_pre_appends_description() {
        let c = BashCapturer;
        let input = serde_json::json!({"command": "ls", "description": "list files"});
        let entry = c.render_pre(&ctx(), &input).unwrap();
        assert_eq!(entry.headline, "Bash  ls  (list files)");
    }

    #[test]
    fn render_post_status_ok_when_no_error() {
        let c = BashCapturer;
        let response = serde_json::json!({"output": "hi"});
        let upd = c.render_post(&ctx(), &Value::Null, &response).unwrap();
        assert_eq!(upd.tool_use_id, "t1");
        assert_eq!(upd.status, EntryStatus::Ok);
    }

    #[test]
    fn render_post_status_error_when_error_field_present() {
        let c = BashCapturer;
        let response = serde_json::json!({"error": "failed"});
        let upd = c.render_post(&ctx(), &Value::Null, &response).unwrap();
        assert_eq!(upd.status, EntryStatus::Error);
    }
}
