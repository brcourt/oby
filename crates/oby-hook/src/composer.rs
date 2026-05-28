//! Compose peer PreToolUse hooks into oby's rewrite.
//!
//! Per Claude Code's docs:
//! > When multiple PreToolUse hooks return `updatedInput` to rewrite a tool's
//! > arguments, the last one to finish wins. Since hooks run in parallel, the
//! > order is non-deterministic. Avoid having more than one hook modify the
//! > same tool's input.
//!
//! Other rewriters (rtk, etc.) coexist commonly in users' settings.json. To
//! keep oby's process-substitution wrap from being overwritten by a peer's
//! emit, we:
//!
//! 1. Find the peer PreToolUse hooks for the same matcher in settings.json.
//! 2. Invoke each ourselves (in array order, sequentially), feeding it the
//!    payload with the running `tool_input` so chained peers see the
//!    accumulated rewrite. Collect each `hookSpecificOutput.updatedInput`.
//! 3. Return that composed `tool_input` to the caller, which wraps it with
//!    oby's own rewrite.
//! 4. The caller adds a brief delay before emitting its own updatedInput so
//!    we reliably win the "last to finish" race.
//!
//! Recursion is prevented by setting `OBS_COMPOSING=1` in the env of peer
//! subprocesses; if oby-hook ever sees that var, it skips composition.

use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Returns `(composed_input, peers_present)`. The boolean is true iff at least
/// one peer hook was found in settings.json for this tool, which the caller
/// uses to decide whether to apply the win-the-race delay.
pub async fn compose_pre_tool_use_input(
    original_payload: &str,
    tool_name: &str,
    initial_tool_input: &Value,
) -> (Value, bool) {
    if std::env::var_os("OBS_COMPOSING").is_some() {
        return (initial_tool_input.clone(), false);
    }
    let Ok(self_exe) = std::env::current_exe() else {
        return (initial_tool_input.clone(), false);
    };
    let self_path = self_exe.to_string_lossy().into_owned();

    let Some(home) = std::env::var_os("HOME") else {
        return (initial_tool_input.clone(), false);
    };
    let settings_path = PathBuf::from(home).join(".claude").join("settings.json");

    let Ok(text) = std::fs::read_to_string(&settings_path) else {
        return (initial_tool_input.clone(), false);
    };
    let Ok(settings) = serde_json::from_str::<Value>(&text) else {
        return (initial_tool_input.clone(), false);
    };

    let Some(hooks) = settings
        .pointer("/hooks/PreToolUse")
        .and_then(|v| v.as_array())
    else {
        return (initial_tool_input.clone(), false);
    };

    let mut working = initial_tool_input.clone();
    let mut peers_present = false;

    for entry in hooks {
        let matcher = entry.get("matcher").and_then(|m| m.as_str()).unwrap_or("");
        if matcher != tool_name {
            continue;
        }
        let Some(hook_objs) = entry.get("hooks").and_then(|h| h.as_array()) else {
            continue;
        };
        for hook_obj in hook_objs {
            let Some(cmd) = hook_obj.get("command").and_then(|c| c.as_str()) else {
                continue;
            };
            if is_self_command(cmd, &self_path) {
                continue;
            }
            peers_present = true;
            if let Some(updated) = invoke_peer(cmd, original_payload, &working).await {
                working = updated;
            }
        }
    }

    (working, peers_present)
}

fn is_self_command(cmd: &str, self_path: &str) -> bool {
    cmd.split_whitespace().next() == Some(self_path)
}

async fn invoke_peer(
    cmd: &str,
    original_payload: &str,
    current_tool_input: &Value,
) -> Option<Value> {
    // Patch payload so the peer sees the running tool_input, not the original.
    let mut patched: Value = serde_json::from_str(original_payload).ok()?;
    if let Some(obj) = patched.as_object_mut() {
        obj.insert("tool_input".into(), current_tool_input.clone());
    }
    let patched_str = patched.to_string();

    let parts: Vec<&str> = cmd.split_whitespace().collect();
    let (program, args) = parts.split_first()?;

    let mut child = Command::new(program)
        .args(args)
        .env("OBS_COMPOSING", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(patched_str.as_bytes()).await.ok()?;
        stdin.shutdown().await.ok()?;
    }

    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .ok()?
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout_text = String::from_utf8_lossy(&output.stdout);
    let parsed: Value = serde_json::from_str(stdout_text.trim()).ok()?;
    parsed.pointer("/hookSpecificOutput/updatedInput").cloned()
}
