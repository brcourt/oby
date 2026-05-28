use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::ExitCode;

const OBSERVED_TOOLS: &[&str] = &["Bash", "Read"];

pub fn run() -> ExitCode {
    match install() {
        Ok(()) => {
            println!("oby: installed hook into ~/.claude/settings.json");
            println!("Run `oby claude` to start an observed session.");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("oby install: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn install() -> Result<()> {
    let path = settings_path()?;
    let mut settings: Value = if path.exists() {
        let s = std::fs::read_to_string(&path).context("reading settings.json")?;
        serde_json::from_str(&s).context("parsing settings.json")?
    } else {
        json!({})
    };

    let oby_hook_path = which::which("oby-hook").context(
        "oby-hook not in PATH — install all four oby binaries before running `oby install`",
    )?;
    let cmd = oby_hook_path.to_string_lossy().to_string();

    let hooks = settings
        .as_object_mut()
        .context("settings.json root must be a JSON object")?
        .entry("hooks")
        .or_insert_with(|| json!({}));
    let hooks_obj = hooks
        .as_object_mut()
        .context("settings.hooks must be an object")?;

    for event in ["PreToolUse", "PostToolUse"] {
        let arr = hooks_obj.entry(event).or_insert_with(|| json!([]));
        let arr = arr.as_array_mut().context("hook event must be an array")?;
        for tool in OBSERVED_TOOLS {
            // Only add if there isn't already an oby-hook entry for this matcher.
            let already = arr.iter().any(|e| {
                e.get("matcher").and_then(|m| m.as_str()) == Some(tool)
                    && e.get("hooks").and_then(|h| h.as_array()).is_some_and(|hs| {
                        hs.iter().any(|h| {
                            h.get("command").and_then(|c| c.as_str()) == Some(cmd.as_str())
                        })
                    })
            });
            if !already {
                arr.push(json!({
                    "matcher": tool,
                    "hooks": [{ "type": "command", "command": cmd }]
                }));
            }
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("creating ~/.claude")?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&settings)?)
        .context("writing settings.json")?;
    Ok(())
}

fn settings_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("$HOME not set")?;
    Ok(PathBuf::from(home).join(".claude").join("settings.json"))
}
