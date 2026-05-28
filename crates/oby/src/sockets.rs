use anyhow::Result;
use oby_core::{ControlMessage, HeaderLine};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::metrics::Metrics;
use crate::ring::AllAgentBuffers;

/// Append a JSON line to `$OBS_WRAPPER_LOG` if that env var is set. Mirrors
/// oby-hook's OBS_HOOK_LOG. Best-effort; never panics, never blocks the
/// wrapper. Off by default (zero overhead).
fn debug_log(event: &str, detail: &str) {
    use std::io::Write;
    let Some(path) = std::env::var_os("OBS_WRAPPER_LOG") else {
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

/// Spawn listener tasks on the given socket dir.
pub async fn spawn_listeners(
    socket_dir: PathBuf,
    buffers: Arc<Mutex<AllAgentBuffers>>,
    metrics: Arc<Mutex<Metrics>>,
) -> Result<()> {
    std::fs::create_dir_all(&socket_dir)?;

    let control_path = socket_dir.join("control.sock");
    let _ = std::fs::remove_file(&control_path);
    let listener = UnixListener::bind(&control_path)?;
    let buffers_for_ctrl = buffers.clone();
    let metrics_for_ctrl = metrics.clone();
    let socket_dir_clone = socket_dir.clone();
    tokio::spawn(async move {
        // Never exit this task on a transient accept() error (EMFILE under
        // bursts of parallel hook invocations is the realistic case). If we
        // exited, every subsequent hook would fail to connect and the feed
        // would silently stop updating.
        loop {
            match listener.accept().await {
                Ok((conn, _)) => {
                    let buffers = buffers_for_ctrl.clone();
                    let metrics = metrics_for_ctrl.clone();
                    let dir = socket_dir_clone.clone();
                    tokio::spawn(async move {
                        let _ = handle_control(conn, buffers, metrics, dir).await;
                    });
                }
                Err(_) => {
                    metrics_for_ctrl.lock().unwrap().accept_errors += 1;
                    debug_log("accept_err", "control");
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    });

    // Pre-create the agent socket for "main" so oby-tee never has to race the
    // wrapper's processing of the first PreToolUse Entry. Subagent sockets are
    // still created lazily in handle_control when their first Entry arrives.
    ensure_agent_listener(&socket_dir, "main", buffers, metrics).await;

    Ok(())
}

async fn handle_control(
    stream: UnixStream,
    buffers: Arc<Mutex<AllAgentBuffers>>,
    metrics: Arc<Mutex<Metrics>>,
    socket_dir: PathBuf,
) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        let line_trim = line.trim();
        if line_trim.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<ControlMessage>(line_trim) else {
            metrics.lock().unwrap().parse_errors += 1;
            debug_log("ctrl_parse_failed", line_trim);
            continue;
        };
        match msg {
            ControlMessage::Entry { entry, .. } => {
                let agent_key = entry.agent_key.clone();
                let tuid = entry.tool_use_id.clone();
                {
                    let mut lock = buffers.lock().unwrap();
                    lock.push_entry(entry);
                }
                metrics.lock().unwrap().entries_received += 1;
                debug_log("entry", &format!("{agent_key}/{tuid}"));
                ensure_agent_listener(&socket_dir, &agent_key, buffers.clone(), metrics.clone())
                    .await;
            }
            ControlMessage::Update { update, .. } => {
                let tuid = update.tool_use_id.clone();
                let status = format!("{:?}", update.status);
                let orphaned = {
                    let mut lock = buffers.lock().unwrap();
                    lock.apply_update(update)
                };
                {
                    let mut m = metrics.lock().unwrap();
                    m.updates_received += 1;
                    if orphaned {
                        m.updates_orphaned += 1;
                    }
                }
                debug_log("update", &format!("{tuid} {status}"));
            }
            ControlMessage::AgentDestroyed { agent_key, .. } => {
                buffers.lock().unwrap().mark_destroyed(&agent_key);
                debug_log("agent_destroyed", &agent_key);
            }
        }
    }
    Ok(())
}

async fn ensure_agent_listener(
    socket_dir: &Path,
    agent_key: &str,
    buffers: Arc<Mutex<AllAgentBuffers>>,
    metrics: Arc<Mutex<Metrics>>,
) {
    let path = socket_dir.join(format!("{agent_key}.sock"));
    if path.exists() {
        return;
    }
    let Ok(listener) = UnixListener::bind(&path) else {
        return;
    };
    let agent_key = agent_key.to_string();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((conn, _)) => {
                    let buffers = buffers.clone();
                    let metrics_for_conn = metrics.clone();
                    let agent_key = agent_key.clone();
                    metrics.lock().unwrap().agent_connections += 1;
                    tokio::spawn(async move {
                        let _ =
                            handle_agent_connection(conn, &agent_key, buffers, metrics_for_conn)
                                .await;
                    });
                }
                Err(_) => {
                    metrics.lock().unwrap().accept_errors += 1;
                    debug_log("accept_err", &format!("agent:{agent_key}"));
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    });
}

async fn handle_agent_connection(
    stream: UnixStream,
    agent_key: &str,
    buffers: Arc<Mutex<AllAgentBuffers>>,
    metrics: Arc<Mutex<Metrics>>,
) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let mut header_line = String::new();
    let n = reader.read_line(&mut header_line).await?;
    if n == 0 {
        return Ok(());
    }
    let Ok(header) = serde_json::from_str::<HeaderLine>(header_line.trim()) else {
        metrics.lock().unwrap().parse_errors += 1;
        debug_log("agent_header_parse_failed", header_line.trim());
        return Ok(());
    };
    debug_log(
        "agent_open",
        &format!("{agent_key}/{}/{}", header.tool_use_id, header.stream),
    );
    let mut buf = vec![0u8; 8 * 1024];
    let mut total = 0usize;
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            debug_log(
                "agent_close",
                &format!("{agent_key}/{} {total}B", header.tool_use_id),
            );
            break;
        }
        total += n;
        metrics.lock().unwrap().agent_bytes += n as u64;
        let mut lock = buffers.lock().unwrap();
        lock.append_live(
            agent_key,
            &header.tool_use_id,
            &header.stream,
            buf[..n].to_vec(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oby_core::{DisplayEntry, EntryBody, EntryStatus};
    use std::time::SystemTime;
    use tempfile::TempDir;
    use tokio::io::AsyncWriteExt;

    fn test_entry() -> DisplayEntry {
        DisplayEntry {
            agent_key: "main".into(),
            tool_use_id: "tu_42".into(),
            tool: "bash".into(),
            timestamp: SystemTime::now(),
            headline: "echo hi".into(),
            body: EntryBody::LiveStream {
                tool_use_id: "tu_42".into(),
            },
            status: EntryStatus::Pending,
        }
    }

    #[tokio::test]
    async fn control_entry_lands_in_buffer() {
        let dir = TempDir::new().unwrap();
        let buffers = Arc::new(Mutex::new(AllAgentBuffers::default()));
        let metrics = Arc::new(Mutex::new(Metrics::default()));
        spawn_listeners(dir.path().to_path_buf(), buffers.clone(), metrics)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut sock = UnixStream::connect(dir.path().join("control.sock"))
            .await
            .unwrap();
        let msg = ControlMessage::entry(test_entry());
        let line = serde_json::to_string(&msg).unwrap() + "\n";
        sock.write_all(line.as_bytes()).await.unwrap();
        drop(sock);

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let lock = buffers.lock().unwrap();
        assert_eq!(lock.get("main").unwrap().entries.len(), 1);
    }

    #[tokio::test]
    async fn agent_socket_receives_live_bytes() {
        let dir = TempDir::new().unwrap();
        let buffers = Arc::new(Mutex::new(AllAgentBuffers::default()));
        let metrics = Arc::new(Mutex::new(Metrics::default()));
        spawn_listeners(dir.path().to_path_buf(), buffers.clone(), metrics)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Push an entry first so the agent socket gets created.
        let mut ctrl = UnixStream::connect(dir.path().join("control.sock"))
            .await
            .unwrap();
        let line = serde_json::to_string(&ControlMessage::entry(test_entry())).unwrap() + "\n";
        ctrl.write_all(line.as_bytes()).await.unwrap();
        drop(ctrl);
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        // Connect as if we're oby-tee and send header + bytes.
        let mut agent_sock = UnixStream::connect(dir.path().join("main.sock"))
            .await
            .unwrap();
        let header = HeaderLine::new("tu_42", "stderr-discarded");
        let header_line = serde_json::to_string(&header).unwrap() + "\n";
        agent_sock.write_all(header_line.as_bytes()).await.unwrap();
        agent_sock.write_all(b"the hidden bytes\n").await.unwrap();
        drop(agent_sock);

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let lock = buffers.lock().unwrap();
        let ring = lock.get("main").unwrap();
        let rec = &ring.entries[0];
        assert_eq!(rec.live.len(), 1);
        assert_eq!(rec.live[0].stream, "stderr-discarded");
        assert_eq!(rec.live[0].bytes, b"the hidden bytes\n");
    }
}
