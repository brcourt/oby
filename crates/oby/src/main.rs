use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "oby",
    version,
    about = "Live, per-agent activity feed for Claude Code"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,

    /// Args passed through to `claude` when no subcommand is used (the common case).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    rest: Vec<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Install the oby-hook into ~/.claude/settings.json.
    Install,
    /// Run claude inside the oby wrapper. (Default if you run `oby claude ...`.)
    Claude {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        rest: Vec<String>,
    },
    /// Test utilities: inject synthetic hook traffic into a running oby session.
    Probe {
        #[command(subcommand)]
        action: ProbeCmd,
    },
}

#[derive(Subcommand)]
enum ProbeCmd {
    /// Print the socket dir of the most recent running oby session.
    Latest,
    /// Inject the v0.1 smoke scenario (entries + chunks + updates) into a
    /// running oby. Validates the wrapper end-to-end without needing claude.
    Smoke {
        /// Override the socket dir. Defaults to the latest running session.
        #[arg(long)]
        socket_dir: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Some(Cmd::Install) => install::run(),
        Some(Cmd::Claude { rest }) => run::run(rest),
        Some(Cmd::Probe { action }) => match action {
            ProbeCmd::Latest => probe::run_latest(),
            ProbeCmd::Smoke { socket_dir } => probe::run_smoke(socket_dir),
        },
        None => {
            // `oby <args...>` with no explicit subcommand — treat as `oby claude <args>`.
            run::run(cli.rest)
        }
    }
}

mod install;
mod key;
mod probe;
mod pty;
mod ring;
mod run;
mod sockets;
mod tui;

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn no_args_parses() {
        let _ = Cli::try_parse_from(["oby"]).unwrap();
    }

    #[test]
    fn install_subcommand_parses() {
        let cli = Cli::try_parse_from(["oby", "install"]).unwrap();
        assert!(matches!(cli.command, Some(Cmd::Install)));
    }

    #[test]
    fn passthrough_args_collected() {
        let cli = Cli::try_parse_from(["oby", "--", "--debug", "foo"]).unwrap();
        assert_eq!(cli.rest, vec!["--debug", "foo"]);
    }
}
