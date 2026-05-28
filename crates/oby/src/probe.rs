//! `oby probe` — synthetic hook traffic for testing the wrapper in isolation.
//!
//! The probe acts as a stand-in for `oby-hook` + `oby-tee` combined. It connects
//! to a running oby session's sockets and sends control messages + agent bytes
//! directly, so you can verify the wrapper's ring / TUI / routing without
//! needing a live Claude Code session.

use anyhow::{Context, Result};
use oby_core::{
    ControlMessage, DisplayEntry, DisplayEntryUpdate, EntryBody, EntryStatus, HeaderLine,
};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

pub fn run_latest() -> ExitCode {
    match find_latest_session() {
        Some(p) => {
            println!("{}", p.display());
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("oby probe: no running oby session found");
            ExitCode::FAILURE
        }
    }
}

pub fn run_smoke(socket_dir_arg: Option<PathBuf>) -> ExitCode {
    let socket_dir = match socket_dir_arg.or_else(find_latest_session) {
        Some(p) => p,
        None => {
            eprintln!("oby probe smoke: no --socket-dir and no running oby session under /tmp/obi or $XDG_RUNTIME_DIR/obi");
            return ExitCode::FAILURE;
        }
    };
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("oby probe smoke: failed to start runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match rt.block_on(smoke_async(socket_dir)) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("oby probe smoke: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn smoke_async(socket_dir: PathBuf) -> Result<()> {
    let control = socket_dir.join("control.sock");
    if !control.exists() {
        return Err(anyhow::anyhow!(
            "control.sock not found at {} — is oby running?",
            control.display()
        ));
    }
    eprintln!("probe: socket dir {}", socket_dir.display());

    let read_id = "probe-read-1";
    send_control(
        &control,
        &ControlMessage::entry(DisplayEntry {
            agent_key: "main".into(),
            tool_use_id: read_id.into(),
            tool: "read".into(),
            timestamp: SystemTime::now(),
            headline: "Read  /etc/hostname  (probe)".into(),
            body: EntryBody::None,
            status: EntryStatus::Pending,
        }),
    )
    .await?;
    eprintln!("probe: → entry Read {read_id}");

    let bash_id = "probe-bash-1";
    send_control(
        &control,
        &ControlMessage::entry(DisplayEntry {
            agent_key: "main".into(),
            tool_use_id: bash_id.into(),
            tool: "bash".into(),
            timestamp: SystemTime::now(),
            headline: "Bash  ls /nonexistent_dir 2>/dev/null; echo done  (probe)".into(),
            body: EntryBody::LiveStream {
                tool_use_id: bash_id.into(),
            },
            status: EntryStatus::Pending,
        }),
    )
    .await?;
    eprintln!("probe: → entry Bash {bash_id}");

    // The wrapper binds the per-agent socket as part of processing the entry.
    // main.sock is pre-bound by spawn_listeners, so this is mostly a safety
    // wait for any subagent sockets that get added in extended scenarios.
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    let main_sock = socket_dir.join("main.sock");
    send_agent_bytes(
        &main_sock,
        bash_id,
        "stderr-discarded",
        b"ls: cannot access '/nonexistent_dir': No such file or directory\n",
    )
    .await?;
    eprintln!("probe: → chunk [stderr-discarded] Bash {bash_id}");

    send_agent_bytes(&main_sock, bash_id, "stdout", b"done\n").await?;
    eprintln!("probe: → chunk [stdout] Bash {bash_id}");

    send_control(
        &control,
        &ControlMessage::update(DisplayEntryUpdate {
            tool_use_id: read_id.into(),
            status: EntryStatus::Ok,
            append_body: Some(EntryBody::Text {
                text: "13 bytes".into(),
            }),
        }),
    )
    .await?;
    eprintln!("probe: → update ok Read {read_id}");

    send_control(
        &control,
        &ControlMessage::update(DisplayEntryUpdate {
            tool_use_id: bash_id.into(),
            status: EntryStatus::Ok,
            append_body: None,
        }),
    )
    .await?;
    eprintln!("probe: → update ok Bash {bash_id}");

    eprintln!("probe: smoke complete — check oby's feed (Ctrl-G).");
    Ok(())
}

async fn send_control(path: &Path, msg: &ControlMessage) -> Result<()> {
    let mut sock = UnixStream::connect(path)
        .await
        .with_context(|| format!("connecting to {}", path.display()))?;
    let line = serde_json::to_string(msg)? + "\n";
    sock.write_all(line.as_bytes()).await?;
    Ok(())
}

async fn send_agent_bytes(
    path: &Path,
    tool_use_id: &str,
    stream: &str,
    bytes: &[u8],
) -> Result<()> {
    let mut sock = UnixStream::connect(path)
        .await
        .with_context(|| format!("connecting to {}", path.display()))?;
    let header = HeaderLine::new(tool_use_id, stream);
    let header_json = serde_json::to_string(&header)? + "\n";
    sock.write_all(header_json.as_bytes()).await?;
    sock.write_all(bytes).await?;
    Ok(())
}

/// Find the most-recently-modified `<base>/obi/<session>` directory that has a
/// live `control.sock`. Returns None if no running session is found.
fn find_latest_session() -> Option<PathBuf> {
    let bases: Vec<PathBuf> = [
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
        Some(PathBuf::from("/tmp")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for base in bases {
        let dir = base.join("obi");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if !path.join("control.sock").exists() {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let Ok(mtime) = meta.modified() else { continue };
            if newest.as_ref().is_none_or(|(t, _)| mtime > *t) {
                newest = Some((mtime, path));
            }
        }
    }
    newest.map(|(_, p)| p)
}
