use anyhow::Result;
use clap::Parser;
use oby_core::HeaderLine;
use std::path::PathBuf;
use std::process::ExitCode;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

#[derive(Parser, Debug)]
#[command(
    version,
    about = "oby-tee: streams stdin into the wrapper's per-agent socket. Fail-open."
)]
pub struct Args {
    #[arg(long)]
    pub agent: String,
    #[arg(long)]
    pub tool_use_id: String,
    #[arg(long)]
    pub stream: String,
    #[arg(long, env = "OBS_SOCKET_DIR")]
    pub socket_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    if let Err(e) = run(args).await {
        // Fail-open: log to stderr (which is itself probably being tee'd!) and exit 0.
        eprintln!("oby-tee: {e:#}");
    }
    ExitCode::SUCCESS
}

async fn run(args: Args) -> Result<()> {
    let socket_dir = args
        .socket_dir
        .ok_or_else(|| anyhow::anyhow!("no socket dir (set OBS_SOCKET_DIR)"))?;
    let socket_path = socket_dir.join(format!("{}.sock", args.agent));

    // Connect, retrying briefly to handle the race where the wrapper hasn't
    // yet bound the agent socket (subagents: socket is created lazily when
    // their first PreToolUse Entry is processed). Total budget ~150ms.
    let mut sock = match connect_with_retry(&socket_path).await {
        Some(s) => s,
        None => {
            drain_stdin().await;
            return Ok(());
        }
    };

    // Write the header line + newline.
    let header = HeaderLine::new(&args.tool_use_id, &args.stream);
    let header_json = serde_json::to_string(&header)? + "\n";
    if sock.write_all(header_json.as_bytes()).await.is_err() {
        drain_stdin().await;
        return Ok(());
    }

    // Stream stdin → socket. Treat any write error as fail-open: keep draining.
    let mut stdin = tokio::io::stdin();
    let mut buf = vec![0u8; 8 * 1024];
    loop {
        let n = match stdin.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        if sock.write_all(&buf[..n]).await.is_err() {
            drain_stdin().await;
            break;
        }
    }
    Ok(())
}

async fn connect_with_retry(path: &std::path::Path) -> Option<UnixStream> {
    // 10 attempts × 15ms = 150ms max. Most connects succeed on the first try.
    for _ in 0..10 {
        if let Ok(s) = UnixStream::connect(path).await {
            return Some(s);
        }
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    }
    None
}

async fn drain_stdin() {
    let mut stdin = tokio::io::stdin();
    let mut buf = vec![0u8; 8 * 1024];
    loop {
        match stdin.read(&mut buf).await {
            Ok(0) => break,
            Ok(_) => continue,
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_arg_set() {
        let args = Args::try_parse_from([
            "oby-tee",
            "--agent",
            "main",
            "--tool-use-id",
            "toolu_01",
            "--stream",
            "stderr-discarded",
            "--socket-dir",
            "/tmp/obs",
        ])
        .unwrap();
        assert_eq!(args.agent, "main");
    }
}
