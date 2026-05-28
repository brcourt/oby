use anyhow::Result;
use oby_core::{ControlMessage, HookContext, HookEvent, RewriteDecision};
use serde_json::Value;
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

mod capturers;
mod composer;
mod env_gate;
mod registry;

#[derive(serde::Deserialize)]
struct RawPayload {
    #[serde(flatten)]
    ctx: HookContext,
    #[serde(default)]
    tool_input: Value,
    #[serde(default)]
    tool_response: Value,
}

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(e) = run().await {
        // Fail-open. Never break the agent.
        eprintln!("oby-hook: {e:#}");
    }
    ExitCode::SUCCESS
}

async fn run() -> Result<()> {
    if !env_gate::is_active() {
        return Ok(());
    }

    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s)?;
    debug_log("recv", &s, "", "");

    let payload: RawPayload = match serde_json::from_str(&s) {
        Ok(p) => p,
        Err(e) => {
            debug_log("parse_failed", &s, "", &e.to_string());
            return Ok(());
        }
    };

    let caps = registry::builtin_capturers();
    let Some(cap) = caps.iter().find(|c| c.tool_name() == payload.ctx.tool_name) else {
        debug_log(
            "no_capturer",
            "",
            &payload.ctx.tool_name,
            &payload.ctx.tool_use_id,
        );
        return Ok(());
    };

    let socket_dir = std::env::var_os("OBS_SOCKET_DIR").map(PathBuf::from);

    match payload.ctx.hook_event_name {
        HookEvent::Pre => {
            if let Some(entry) = cap.render_pre(&payload.ctx, &payload.tool_input) {
                debug_log(
                    "pre_entry",
                    &entry.headline,
                    &payload.ctx.tool_name,
                    &payload.ctx.tool_use_id,
                );
                send_to_wrapper(&socket_dir, ControlMessage::entry(entry)).await;
            } else {
                debug_log(
                    "pre_no_entry",
                    "",
                    &payload.ctx.tool_name,
                    &payload.ctx.tool_use_id,
                );
            }
            // Compose with peer PreToolUse hooks first (rtk, etc.) so our
            // wrap is built on top of their transformations rather than
            // overwriting them in CC's last-to-finish race.
            let (composed_input, peers_present) = composer::compose_pre_tool_use_input(
                &s,
                &payload.ctx.tool_name,
                &payload.tool_input,
            )
            .await;
            if peers_present {
                let preview = composed_input
                    .get("command")
                    .and_then(|c| c.as_str())
                    .map(|s| s.chars().take(160).collect::<String>())
                    .unwrap_or_default();
                debug_log(
                    "compose_peers",
                    &preview,
                    &payload.ctx.tool_name,
                    &payload.ctx.tool_use_id,
                );
            }

            if let RewriteDecision::Rewrite(new_input) =
                cap.pre_rewrite(&payload.ctx, &composed_input)
            {
                let preview = new_input
                    .get("command")
                    .and_then(|c| c.as_str())
                    .map(|s| s.chars().take(160).collect::<String>())
                    .unwrap_or_default();
                debug_log(
                    "pre_rewrite",
                    &preview,
                    &payload.ctx.tool_name,
                    &payload.ctx.tool_use_id,
                );
                // Win CC's last-to-finish race when peers exist. Without this
                // delay, a peer (rtk) often finishes after us in real-world
                // timing because it's a simpler process — its updatedInput
                // overwrites ours and oby-tee never makes it into the
                // executed command. Override the delay with
                // OBS_COMPOSE_DELAY_MS if 200ms is too long.
                if peers_present {
                    let delay_ms = std::env::var("OBS_COMPOSE_DELAY_MS")
                        .ok()
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(200);
                    if delay_ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                }
                emit_hook_decision(&new_input)?;
            } else {
                debug_log(
                    "pre_passthrough",
                    "",
                    &payload.ctx.tool_name,
                    &payload.ctx.tool_use_id,
                );
            }
        }
        HookEvent::Post => {
            if let Some(update) =
                cap.render_post(&payload.ctx, &payload.tool_input, &payload.tool_response)
            {
                debug_log(
                    "post_update",
                    &format!("{:?}", update.status),
                    &payload.ctx.tool_name,
                    &payload.ctx.tool_use_id,
                );
                send_to_wrapper(&socket_dir, ControlMessage::update(update)).await;
            } else {
                debug_log(
                    "post_no_update",
                    "",
                    &payload.ctx.tool_name,
                    &payload.ctx.tool_use_id,
                );
            }
        }
    }
    Ok(())
}

/// Append a debug line to `$OBS_HOOK_LOG` if that env var names a file path.
/// Best-effort; never fails the hook. Each line is JSON for trivial filtering
/// with `jq`. `detail` is free-form per-event context (headline, rewrite preview,
/// parse error, etc.).
fn debug_log(event: &str, detail: &str, tool: &str, tool_use_id: &str) {
    use std::io::Write;
    let Some(path) = std::env::var_os("OBS_HOOK_LOG") else {
        return;
    };
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let line = serde_json::json!({
        "ts": ts,
        "pid": std::process::id(),
        "event": event,
        "tool": tool,
        "tool_use_id": tool_use_id,
        "detail": detail,
    })
    .to_string();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{line}");
    }
}

async fn send_to_wrapper(socket_dir: &Option<PathBuf>, msg: ControlMessage) {
    let Some(dir) = socket_dir else { return };
    let path = dir.join("control.sock");
    let Ok(mut sock) = UnixStream::connect(&path).await else {
        return;
    };
    let line = match serde_json::to_string(&msg) {
        Ok(s) => s,
        Err(_) => return,
    };
    let _ = sock.write_all(line.as_bytes()).await;
    let _ = sock.write_all(b"\n").await;
}

fn emit_hook_decision(new_input: &Value) -> Result<()> {
    let envelope = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "updatedInput": new_input,
        }
    });
    println!("{}", serde_json::to_string(&envelope)?);
    Ok(())
}
