use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtyPair, PtySize};
use std::sync::mpsc::Receiver;

pub struct PtySession {
    pub pair: PtyPair,
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
}

/// Spawn `claude` (with the supplied args) inside a fresh pty.
pub fn spawn_claude(rest: Vec<String>, cols: u16, rows: u16) -> Result<PtySession> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("openpty failed")?;

    let mut cmd = CommandBuilder::new("claude");
    for arg in rest {
        cmd.arg(arg);
    }
    // CommandBuilder defaults to $HOME (or /); inherit the caller's cwd so
    // `oby claude` from inside a project starts claude in that project.
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }
    let child = pair
        .slave
        .spawn_command(cmd)
        .context("failed to spawn claude")?;
    Ok(PtySession { pair, child })
}

/// Wait on child in a background thread; returns a receiver that fires when claude exits.
pub fn watch_child(
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
) -> Receiver<portable_pty::ExitStatus> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Ok(st) = child.wait() {
            let _ = tx.send(st);
        }
    });
    rx
}
