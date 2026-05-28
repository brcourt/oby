//! Black-box test: `oby-hook --version` prints the package version on stdout.

use std::process::Command;

fn binary() -> std::path::PathBuf {
    // Cargo's binary path. `env!("CARGO_BIN_EXE_oby-hook")` is set during
    // integration test compilation.
    env!("CARGO_BIN_EXE_oby-hook").into()
}

#[test]
fn version_flag_prints_version_and_exits_zero() {
    let out = Command::new(binary())
        .arg("--version")
        .output()
        .expect("failed to spawn oby-hook");
    assert!(out.status.success(), "exit code: {:?}", out.status);
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "stdout should contain version; got: {stdout:?}"
    );
    assert!(
        stdout.starts_with("oby-hook "),
        "stdout should start with 'oby-hook '; got: {stdout:?}"
    );
}

#[test]
fn short_version_flag_works_too() {
    let out = Command::new(binary())
        .arg("-V")
        .output()
        .expect("failed to spawn oby-hook");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")));
}
