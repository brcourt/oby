use anyhow::Result;
use oby_core::{ControlMessage, HookContext, HookEvent, RewriteDecision};
use serde_json::Value;
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

mod capturers;
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
    let payload: RawPayload = match serde_json::from_str(&s) {
        Ok(p) => p,
        Err(_) => return Ok(()),
    };

    let caps = registry::builtin_capturers();
    let Some(cap) = caps.iter().find(|c| c.tool_name() == payload.ctx.tool_name) else {
        return Ok(());
    };

    let socket_dir = std::env::var_os("OBS_SOCKET_DIR").map(PathBuf::from);

    match payload.ctx.hook_event_name {
        HookEvent::Pre => {
            if let Some(entry) = cap.render_pre(&payload.ctx, &payload.tool_input) {
                send_to_wrapper(&socket_dir, ControlMessage::entry(entry)).await;
            }
            if let RewriteDecision::Rewrite(new_input) =
                cap.pre_rewrite(&payload.ctx, &payload.tool_input)
            {
                emit_hook_decision(&new_input)?;
            }
        }
        HookEvent::Post => {
            if let Some(update) =
                cap.render_post(&payload.ctx, &payload.tool_input, &payload.tool_response)
            {
                send_to_wrapper(&socket_dir, ControlMessage::update(update)).await;
            }
        }
    }
    Ok(())
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
