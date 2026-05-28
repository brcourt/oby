use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    version,
    about = "oby-tee: streams stdin into the wrapper's per-agent socket. Fail-open."
)]
pub struct Args {
    /// Agent routing key (typically agent_id, or 'main' for the main agent).
    #[arg(long)]
    pub agent: String,

    /// Correlation id provided by the rewriter (the CC tool_use_id).
    #[arg(long)]
    pub tool_use_id: String,

    /// Sub-stream label: stdout, stderr, stderr-discarded, stdout-piped, etc.
    #[arg(long)]
    pub stream: String,

    /// Socket directory. Defaults to $OBS_SOCKET_DIR.
    #[arg(long, env = "OBS_SOCKET_DIR")]
    pub socket_dir: Option<PathBuf>,
}

fn main() {
    // Filled in next task.
    let _ = Args::parse();
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

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
        assert_eq!(args.tool_use_id, "toolu_01");
        assert_eq!(args.stream, "stderr-discarded");
        assert_eq!(
            args.socket_dir.as_deref().map(|p| p.to_str().unwrap()),
            Some("/tmp/obs")
        );
    }
}
