use std::io::Write;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tempfile::TempDir;

fn binary_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.push("target");
    p.push(if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    });
    p.push("oby-hook");
    p
}

const READ_PAYLOAD: &str = r#"{
    "session_id": "s","transcript_path":"/t","cwd":"/c",
    "hook_event_name":"PreToolUse","tool_name":"Read","tool_use_id":"tu_42",
    "tool_input": {"file_path": "/x/foo.ts"}
}"#;

#[test]
fn env_gate_off_means_silent_passthrough() {
    let mut child = Command::new(binary_path())
        .env_remove("OBS_ACTIVE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(READ_PAYLOAD.as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(
        out.stdout.is_empty(),
        "no rewrite output when env-gated off"
    );
}

#[test]
fn read_dispatch_sends_entry_to_control_socket() {
    let dir = TempDir::new().unwrap();
    let socket_path = dir.path().join("control.sock");

    let listener = UnixListener::bind(&socket_path).unwrap();
    let server = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().unwrap();
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut conn, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    });

    let mut child = Command::new(binary_path())
        .env("OBS_ACTIVE", "1")
        .env("OBS_SOCKET_DIR", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(READ_PAYLOAD.as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());

    let server_received = server.join().unwrap();
    assert!(server_received.contains("\"kind\":\"entry\""));
    assert!(server_received.contains("\"tool_use_id\":\"tu_42\""));
    assert!(server_received.contains("\"headline\":\"Read  /x/foo.ts\""));
}

#[test]
fn bash_dispatch_emits_rewrite_json_to_stdout() {
    let dir = TempDir::new().unwrap();
    let payload = r#"{
        "session_id": "s","transcript_path":"/t","cwd":"/c",
        "hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"tu_b",
        "tool_input": {"command": "ls 2>/dev/null"}
    }"#;
    let mut child = Command::new(binary_path())
        .env("OBS_ACTIVE", "1")
        .env("OBS_SOCKET_DIR", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("\"hookSpecificOutput\""));
    assert!(stdout.contains("\"updatedInput\""));
    assert!(stdout.contains("--stream stderr-discarded"));
}
