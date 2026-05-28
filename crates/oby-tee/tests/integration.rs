use std::io::Write;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tempfile::TempDir;

fn binary_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // out of oby-tee/
    p.pop(); // out of crates/
    p.push("target");
    p.push(if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    });
    p.push("oby-tee");
    p
}

#[test]
fn streams_stdin_to_socket_with_header() {
    let dir = TempDir::new().unwrap();
    let socket_path = dir.path().join("main.sock");

    let listener = UnixListener::bind(&socket_path).unwrap();
    let server = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().unwrap();
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut conn, &mut buf).unwrap();
        buf
    });

    let mut child = Command::new(binary_path())
        .args([
            "--agent",
            "main",
            "--tool-use-id",
            "toolu_T",
            "--stream",
            "stderr-discarded",
        ])
        .env("OBS_SOCKET_DIR", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"hello-from-stdin\n")
        .unwrap();
    drop(child.stdin.take()); // close stdin → EOF
    let status = child.wait().unwrap();
    assert!(status.success(), "oby-tee must always exit 0");

    let bytes = server.join().unwrap();
    let s = String::from_utf8(bytes).unwrap();
    let (header, body) = s.split_once('\n').unwrap();
    assert!(header.contains("\"tool_use_id\":\"toolu_T\""));
    assert!(header.contains("\"stream\":\"stderr-discarded\""));
    assert_eq!(body, "hello-from-stdin\n");
}

#[test]
fn fail_open_when_socket_missing() {
    let dir = TempDir::new().unwrap(); // empty — no socket
    let mut child = Command::new(binary_path())
        .args([
            "--agent",
            "main",
            "--tool-use-id",
            "toolu_X",
            "--stream",
            "stderr",
        ])
        .env("OBS_SOCKET_DIR", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"some bytes\n")
        .unwrap();
    drop(child.stdin.take());
    let status = child.wait().unwrap();
    assert!(status.success(), "must exit 0 even with no listener");
}
