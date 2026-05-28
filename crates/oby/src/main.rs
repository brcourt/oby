use clap::{Parser, Subcommand};
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
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Some(Cmd::Install) => install::run(),
        Some(Cmd::Claude { rest }) => run::run(rest),
        None => {
            // `oby <args...>` with no explicit subcommand — treat as `oby claude <args>`.
            run::run(cli.rest)
        }
    }
}

mod install;
mod key;
mod pty;
mod ring;
mod run;
mod sockets;

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
