//! `prov`: the command line.

mod agent_hook;
mod archive;
mod check;
mod context;
mod gate;
mod git_hook;
mod sweep;

use std::io::Read;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "prov", version = concat!(env!("CARGO_PKG_VERSION"), "+", env!("PROV_BUILD_SHA")), about = "Provenance for repositories where AI agents and humans commit")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Handles an agent or git event. For the hook shims only.
    Hook {
        /// `git`, or the name of an agent adapter, for example `claude-code`.
        adapter: String,
        /// The event or the git hook name.
        event: String,
        /// The arguments that git gives the hook.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Applies the rules of the merge gate without a merge.
    Check {
        /// An agent branch, or `<base>..<tip>` for commits on the human branch.
        target: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Runs the archive step of a Closing session. Started by `prov` itself.
    #[command(hide = true)]
    ArchiveSession { session: String },
}

fn main() -> ExitCode {
    // Hooks have a time limit, and building the full command-line parser
    // costs more than the rest of a hook. `prov hook` is parsed by hand.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [hook, adapter, event, rest @ ..] if hook == "hook" && !adapter.starts_with('-') && !event.starts_with('-') => {
            run_hook(adapter, event, rest)
        }
        _ => match Cli::parse().command {
            Cmd::Hook { adapter, event, args } => run_hook(&adapter, &event, &args),
            Cmd::Check { target, json } => check::run(target.as_deref(), json),
            Cmd::ArchiveSession { session } => archive::run(&session).map(|()| true),
        },
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("prov: error: {e:#}");
            // Never 2: Claude Code treats exit code 2 of a hook as a request to block.
            ExitCode::from(1)
        }
    }
}

fn run_hook(adapter: &str, event: &str, args: &[String]) -> anyhow::Result<bool> {
    if adapter == "git" {
        return git_hook::run(event, args);
    }
    let mut payload = String::new();
    std::io::stdin().read_to_string(&mut payload)?;
    agent_hook::run(adapter, event, &payload).map(|()| true)
}
