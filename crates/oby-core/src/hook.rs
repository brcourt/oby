use serde::Deserialize;
use std::path::PathBuf;

/// Mirrors the PreToolUse / PostToolUse payload Claude Code sends to a hook on stdin.
/// Field schema empirically verified against CC 2.1.142 (see docs/architecture.md, Appendix A).
#[derive(Debug, Clone, Deserialize)]
pub struct HookContext {
    pub session_id: String,
    pub transcript_path: PathBuf,
    pub cwd: PathBuf,
    pub hook_event_name: HookEvent,
    pub tool_name: String,
    pub tool_use_id: String,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub effort: Option<EffortLevel>,
    /// Present iff the call came from inside a subagent. This is the routing key.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Subagent type name (e.g. "general-purpose"). Present iff `agent_id` is.
    #[serde(default)]
    pub agent_type: Option<String>,
}

impl HookContext {
    /// `"main"` for the main agent, the `agent_id` otherwise. Stable per-agent routing key.
    pub fn agent_key(&self) -> &str {
        self.agent_id.as_deref().unwrap_or("main")
    }
}

#[derive(Debug, Copy, Clone, Deserialize, PartialEq, Eq)]
pub enum HookEvent {
    #[serde(rename = "PreToolUse")]
    Pre,
    #[serde(rename = "PostToolUse")]
    Post,
    /// Fires when a tool call returned an error. CC routes failures here
    /// instead of `PostToolUse`, so without an explicit handler the entry
    /// would stay pending on any failed Read/Bash/etc.
    #[serde(rename = "PostToolUseFailure")]
    PostFailure,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EffortLevel {
    pub level: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from the empirical probe (see docs/architecture.md Appendix A).
    const MAIN_AGENT_PAYLOAD: &str = r#"{
        "session_id": "a9db5455-5a02-44b1-b807-0bf79d80e6b1",
        "transcript_path": "/Users/brandon/.claude/projects/-private-tmp-ccprobe/a9db5455-5a02-44b1-b807-0bf79d80e6b1.jsonl",
        "cwd": "/private/tmp/ccprobe",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_use_id": "toolu_01",
        "permission_mode": "bypassPermissions",
        "effort": {"level": "medium"}
    }"#;

    const SUBAGENT_PAYLOAD: &str = r#"{
        "session_id": "a9db5455-5a02-44b1-b807-0bf79d80e6b1",
        "transcript_path": "/Users/brandon/.claude/projects/-private-tmp-ccprobe/a9db5455-5a02-44b1-b807-0bf79d80e6b1.jsonl",
        "cwd": "/private/tmp/ccprobe",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_use_id": "toolu_02",
        "agent_id": "a56e70ccdc442bf74",
        "agent_type": "general-purpose"
    }"#;

    #[test]
    fn deserializes_main_agent_payload() {
        let ctx: HookContext = serde_json::from_str(MAIN_AGENT_PAYLOAD).unwrap();
        assert_eq!(ctx.tool_name, "Bash");
        assert_eq!(ctx.hook_event_name, HookEvent::Pre);
        assert!(ctx.agent_id.is_none());
        assert!(ctx.agent_type.is_none());
        assert_eq!(ctx.agent_key(), "main");
    }

    #[test]
    fn deserializes_subagent_payload() {
        let ctx: HookContext = serde_json::from_str(SUBAGENT_PAYLOAD).unwrap();
        assert_eq!(ctx.agent_id.as_deref(), Some("a56e70ccdc442bf74"));
        assert_eq!(ctx.agent_type.as_deref(), Some("general-purpose"));
        assert_eq!(ctx.agent_key(), "a56e70ccdc442bf74");
    }
}
