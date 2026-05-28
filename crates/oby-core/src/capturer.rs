use crate::{DisplayEntry, DisplayEntryUpdate, HookContext, HookEvent};
use serde_json::Value;

#[derive(Debug)]
pub enum RewriteDecision {
    Passthrough,
    /// New `tool_input` to be marshaled by oby-hook into `hookSpecificOutput.updatedInput`.
    /// Keeping this as a raw Value lets capturers stay independent of CC's exact hook-output schema.
    Rewrite(Value),
}

/// The contribution API. Every observed CC tool gets one Capturer impl in this crate's source tree.
pub trait Capturer: Send + Sync + 'static {
    /// Stable identifier; matches `[capture.<name>]` in config and the filter UI label.
    fn name(&self) -> &'static str;

    /// CC tool name to match (e.g. `"Bash"`, `"Read"`).
    fn tool_name(&self) -> &'static str;

    /// Which hook events this capturer wants. Default: both Pre and Post.
    fn subscribes_to(&self) -> &'static [HookEvent] {
        &[HookEvent::Pre, HookEvent::Post]
    }

    /// Optional rewrite. Default: passthrough. Only the Bash capturer overrides this in v0.1.
    fn pre_rewrite(&self, _ctx: &HookContext, _input: &Value) -> RewriteDecision {
        RewriteDecision::Passthrough
    }

    /// Render a PreToolUse event. Return `None` to suppress (e.g. for noisy calls).
    fn render_pre(&self, ctx: &HookContext, input: &Value) -> Option<DisplayEntry>;

    /// Render a PostToolUse event. Default: no update (Pre-only capturers).
    /// The wrapper correlates Pre↔Post by `tool_use_id` and applies this update.
    fn render_post(
        &self,
        _ctx: &HookContext,
        _input: &Value,
        _response: &Value,
    ) -> Option<DisplayEntryUpdate> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntryBody, EntryStatus};
    use std::time::SystemTime;

    struct NoOpCapturer;
    impl Capturer for NoOpCapturer {
        fn name(&self) -> &'static str {
            "noop"
        }
        fn tool_name(&self) -> &'static str {
            "NoSuchTool"
        }
        fn render_pre(&self, ctx: &HookContext, _: &Value) -> Option<DisplayEntry> {
            Some(DisplayEntry {
                agent_key: ctx.agent_key().to_string(),
                tool_use_id: ctx.tool_use_id.clone(),
                tool: "noop".to_string(),
                timestamp: SystemTime::now(),
                headline: "noop".into(),
                body: EntryBody::None,
                status: EntryStatus::Pending,
            })
        }
    }

    #[test]
    fn trait_is_object_safe() {
        let _: Box<dyn Capturer> = Box::new(NoOpCapturer);
    }

    #[test]
    fn default_pre_rewrite_is_passthrough() {
        let c = NoOpCapturer;
        let ctx: HookContext = serde_json::from_str(
            r#"{"session_id":"s","transcript_path":"/t","cwd":"/c","hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"t1"}"#,
        ).unwrap();
        match c.pre_rewrite(&ctx, &Value::Null) {
            RewriteDecision::Passthrough => {}
            _ => panic!("expected Passthrough"),
        }
    }
}
