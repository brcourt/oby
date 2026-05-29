//! Integration: run the rewritten command through actual bash and zsh
//! shells and verify xtrace bytes land in the xtrace stream, not in
//! stderr.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn binary() -> PathBuf {
    env!("CARGO_BIN_EXE_oby-hook").into()
}

/// Resolve a shell name to its absolute path via `which`, returning None if not found.
fn resolve_shell(name: &str) -> Option<String> {
    let out = Command::new("which").arg(name).output().ok()?;
    if out.status.success() {
        Some(String::from_utf8(out.stdout).ok()?.trim().to_string())
    } else {
        None
    }
}

/// Build a temp dir, drop a fake oby-tee in it that logs (stream, stdin)
/// pairs, and return (dir, modified PATH).
fn fake_oby_tee_env() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin_dir = dir.path();
    let log_dir = bin_dir.join("log");
    fs::create_dir_all(&log_dir).unwrap();

    let stub = bin_dir.join("oby-tee");
    // Args we care about: --stream NAME. Find that and use as filename.
    let script = format!(
        r#"#!/bin/sh
stream=""
while [ $# -gt 0 ]; do
  case "$1" in
    --stream)
      stream="$2"; shift 2 ;;
    *)
      shift ;;
  esac
done
[ -z "$stream" ] && stream="unknown"
cat >> "{log}/$stream.txt"
"#,
        log = log_dir.display()
    );
    fs::write(&stub, &script).unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();

    let new_path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    (dir, new_path)
}

fn rewrite_via_hook(path: &str, shell_bin: &str, command: &str) -> String {
    let payload = serde_json::json!({
        "session_id": "test-session",
        "transcript_path": "/tmp/oby-test.jsonl",
        "cwd": "/tmp",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_use_id": "test-tool-use",
        "tool_input": { "command": command }
    });
    let mut child = Command::new(binary())
        .env("OBS_ACTIVE", "1")
        .env("OBS_SOCKET_DIR", "/tmp/oby-no-sock")
        .env("PATH", path)
        // Set SHELL so detect() picks the right wrap shape.
        .env("SHELL", shell_bin)
        // Prevent peer-hook composition: we want a clean single-level wrap.
        // Without this, the composer may pick up the real oby-hook from
        // ~/.claude/settings.json and produce a double-wrapped command.
        .env("OBS_COMPOSING", "1")
        // Ensure BASH_VERSION and ZSH_VERSION are not inherited (could confuse detect()).
        .env_remove("BASH_VERSION")
        .env_remove("ZSH_VERSION")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn oby-hook");
    {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(payload.to_string().as_bytes()).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let envelope: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|_| panic!("hook stdout was not JSON: {stdout:?}"));
    envelope["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .expect("rewritten command missing")
        .to_string()
}

fn run_in_shell(shell: &str, path: &str, command: &str) {
    let status = Command::new(shell)
        .arg("-c")
        .arg(command)
        .env("PATH", path)
        .status()
        .expect("spawn shell");
    assert!(status.success(), "{shell} exit: {status:?}");
}

fn bash_supports_xtracefd() -> bool {
    let output = Command::new("bash")
        .arg("-c")
        .arg("echo $BASH_VERSINFO")
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let v = String::from_utf8_lossy(&out.stdout);
            v.trim().parse::<u32>().is_ok_and(|major| major >= 4)
        }
        _ => false,
    }
}

#[test]
fn bash_xtrace_lands_in_xtrace_stream() {
    let Some(bash_path) = resolve_shell("bash") else {
        eprintln!("bash not available; skipping");
        return;
    };
    if Command::new(&bash_path)
        .arg("-c")
        .arg("true")
        .status()
        .map_or(true, |s| !s.success())
    {
        eprintln!("bash not available; skipping");
        return;
    }
    if !bash_supports_xtracefd() {
        eprintln!("bash version < 4.1 (no BASH_XTRACEFD); skipping");
        return;
    }
    let (dir, path) = fake_oby_tee_env();
    let wrapped = rewrite_via_hook(&path, &bash_path, "echo hello");
    run_in_shell(&bash_path, &path, &wrapped);

    let xtrace_log = dir.path().join("log/xtrace.txt");
    let xtrace = fs::read_to_string(&xtrace_log).expect("xtrace log");
    assert!(
        xtrace.contains("echo hello"),
        "bash xtrace missing 'echo hello'; got: {xtrace:?}"
    );
    let stderr_log = dir.path().join("log/stderr.txt");
    let stderr = fs::read_to_string(&stderr_log).unwrap_or_default();
    assert!(
        !stderr.contains("+ echo hello"),
        "bash trace must NOT appear in stderr; got: {stderr:?}"
    );
}

#[test]
fn zsh_xtrace_lands_in_xtrace_stream() {
    let Some(zsh_path) = resolve_shell("zsh") else {
        eprintln!("zsh not available; skipping");
        return;
    };
    if Command::new(&zsh_path)
        .arg("-c")
        .arg("true")
        .status()
        .map_or(true, |s| !s.success())
    {
        eprintln!("zsh not available; skipping");
        return;
    }
    let (dir, path) = fake_oby_tee_env();
    let wrapped = rewrite_via_hook(&path, &zsh_path, "echo hello");
    run_in_shell(&zsh_path, &path, &wrapped);

    let xtrace_log = dir.path().join("log/xtrace.txt");
    let xtrace = fs::read_to_string(&xtrace_log).expect("xtrace log");
    // Under zsh the trace is +(eval):N> echo hello or similar zsh-native form.
    assert!(
        xtrace.contains("echo hello"),
        "zsh xtrace missing 'echo hello'; got: {xtrace:?}"
    );
    let stderr_log = dir.path().join("log/stderr.txt");
    let stderr = fs::read_to_string(&stderr_log).unwrap_or_default();
    // The xtrace lines start with +; real stderr would be from the command,
    // and `echo hello` produces no stderr. So the stderr log should NOT
    // contain `echo hello` as part of a trace line.
    assert!(
        !stderr.contains("echo hello"),
        "zsh trace must NOT appear in stderr; got: {stderr:?}"
    );
}
