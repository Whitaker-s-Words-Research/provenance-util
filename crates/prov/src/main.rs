//! `prov`: the command line.

mod agent_hook;
mod approve;
mod archive;
mod check;
mod complete;
mod completions;
mod context;
mod copy_check;
mod cp;
mod discard;
mod doctor;
mod finish;
mod gate;
mod git_hook;
mod init;
mod procs;
mod schema;
mod session;
mod start;
mod sweep;
mod trace;

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::engine::ArgValueCandidates;

#[derive(Parser)]
#[command(name = "prov", version = concat!(env!("CARGO_PKG_VERSION"), "+", env!("PROV_BUILD_SHA")), about = "Provenance for repositories where AI agents and humans commit")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Sets up this clone: the config file, the hook shims, the agent settings and the git options.
    Init {
        /// An agent in use. Default: the agents of the config file, or `claude`.
        #[arg(long = "agent", value_name = "NAME")]
        agents: Vec<String>,
        /// Replace a file or a hook that has other content.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Checks the setup of this clone.
    Doctor {
        /// Also run `git-ai debug`.
        #[arg(long)]
        deep: bool,
        #[arg(long)]
        json: bool,
    },
    /// Applies the rules of the merge gate without a merge.
    Check {
        /// An agent branch, or `<base>..<tip>` for commits that are already
        /// on a human branch.
        #[arg(add = ArgValueCandidates::new(complete::agent_branches))]
        target: Option<String>,
        /// The branch that the agent branch will merge into. Default: the
        /// branch that is checked out.
        #[arg(long, value_name = "BRANCH", add = ArgValueCandidates::new(complete::branches))]
        into: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Creates a worktree on a new agent branch and starts the agent in it.
    Start(StartArgs),
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
    /// Shows sessions, closes one, or deletes the record of one.
    Session {
        #[command(subcommand)]
        action: SessionCmd,
    },
    /// Removes the worktree of a session. The branch stays.
    Finish {
        /// A session ID, or the branch of the worktree.
        #[arg(add = ArgValueCandidates::new(complete::finish_targets))]
        target: String,
        /// Remove the worktree also while a session in it is open. Each open session is closed first.
        #[arg(long = "override")]
        override_: bool,
        /// Passed to `git worktree remove`: remove a worktree with changes that are not committed.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Deletes an agent branch that will not be integrated: its worktree, the branch, the session records and the transcript archives.
    Discard {
        /// A session ID, or an agent branch.
        #[arg(add = ArgValueCandidates::new(complete::discard_targets))]
        target: String,
        /// Delete. Without it, `prov discard` only prints what it would delete.
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Records a human decision about the authorship of paths.
    Approve(ApproveArgs),
    /// Prints the line that turns on tab completion, or adds it to your shell's startup file.
    Completions {
        /// `bash`, `zsh` or `fish`. Default: the shell of `$SHELL`.
        #[arg(long, value_parser = ["bash", "zsh", "fish"])]
        shell: Option<String>,
        /// Add the line to the startup file, if it is not there yet.
        #[arg(long)]
        install: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Agent Trace records.
    Trace {
        #[command(subcommand)]
        action: TraceCmd,
    },
    /// Copies files from a commit of a local clone, with an author for every line.
    Cp(CpArgs),
    /// Runs the deferred check of a copy. Started by `prov` itself.
    #[command(hide = true)]
    CopyCheck { id: String },
    /// Runs the archive step of a Closing session. Started by `prov` itself.
    #[command(hide = true)]
    ArchiveSession {
        session: String,
        /// The generation of the session that this process belongs to.
        #[arg(long)]
        generation: Option<u64>,
    },
}

#[derive(Args)]
struct StartArgs {
    /// The agent, for example `claude`.
    #[arg(add = ArgValueCandidates::new(complete::agents))]
    agent: String,
    /// The `<name>` part of the branch name. Default: 4 random hex characters.
    #[arg(long, value_name = "SLUG")]
    name: Option<String>,
    /// The full branch name. It must match `agent/**`.
    #[arg(long, value_name = "NAME")]
    branch: Option<String>,
    /// The start point of the new branch. Default: the branch that is checked out.
    #[arg(long, value_name = "REF", add = ArgValueCandidates::new(complete::branches))]
    from: Option<String>,
    /// The directory that holds the worktree. The worktree is a new
    /// directory in it, named after the branch. A leading `~` is the home
    /// directory. Default: `../<repo>.worktrees/`.
    #[arg(long, value_name = "DIR", value_hint = clap::ValueHint::DirPath)]
    path: Option<PathBuf>,
    /// Create the worktree, print its path, and do not start the agent.
    #[arg(long)]
    no_launch: bool,
    /// Open the worktree of that session and start the agent with its resume option.
    #[arg(long, value_name = "SESSION", add = ArgValueCandidates::new(complete::sessions))]
    resume: Option<String>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    json: bool,
    /// Arguments for the agent, after `--`.
    #[arg(last = true)]
    agent_args: Vec<String>,
}

#[derive(Subcommand)]
enum SessionCmd {
    /// Lists the sessions that have a record.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Shows one session.
    Show {
        #[arg(add = ArgValueCandidates::new(complete::sessions))]
        session: String,
        #[arg(long)]
        json: bool,
    },
    /// Moves an Open session to Closing and archives it. For a Failed session: runs the archive step again.
    Close {
        #[arg(add = ArgValueCandidates::new(complete::sessions))]
        session: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Deletes the record of a session. The transcript archive stays.
    Drop {
        #[arg(add = ArgValueCandidates::new(complete::sessions))]
        session: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct ApproveArgs {
    #[arg(required = true)]
    paths: Vec<PathBuf>,
    /// While a merge is in progress: the lines keep their git-ai attribution.
    #[arg(long, group = "mode")]
    merge: bool,
    /// Before your own commit: you wrote the changes, or a generator that you ran.
    #[arg(long, group = "mode")]
    human: bool,
    /// Before your own commit: an AI wrote the changes.
    #[arg(long, group = "mode")]
    ai: bool,
    /// With --ai: the tool. Default: `unknown`.
    #[arg(long, value_name = "TOOL")]
    agent: Option<String>,
    /// With --ai: the model. Default: `unknown`.
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct CpArgs {
    /// The path of a local clone: the source repository.
    #[arg(long, value_name = "REPO")]
    from: Option<PathBuf>,
    /// The source commit. Default: `HEAD` of the source.
    #[arg(long, value_name = "REV")]
    rev: Option<String>,
    /// The paths in the source, relative to its root, and then the destination.
    #[arg(value_name = "PATH")]
    paths: Vec<String>,
    /// The author of every line: `human` or `ai`. The source's notes are ignored.
    #[arg(long, value_name = "AUTHOR")]
    all: Option<String>,
    /// The author of every line that has none from the source: `human` or `ai`.
    #[arg(long, value_name = "AUTHOR")]
    missing: Option<String>,
    /// With `ai`: the tool. Default: `unknown`.
    #[arg(long, value_name = "TOOL")]
    agent: Option<String>,
    /// With `ai`: the model. Default: `unknown`.
    #[arg(long)]
    model: Option<String>,
    /// With `--all ai`: the conversation ID. Default: the plan ID.
    #[arg(long, value_name = "ID")]
    session: Option<String>,
    /// Write the plan to a file and stop.
    #[arg(long, value_name = "FILE")]
    plan: Option<PathBuf>,
    /// Apply a plan file.
    #[arg(long, value_name = "FILE")]
    apply: Option<PathBuf>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum TraceCmd {
    /// Writes Agent Trace 0.1.0 records as JSON Lines, one record for each commit with a note.
    Export {
        /// A git range, for example `main..HEAD`. Default: the branch that is checked out.
        #[arg(add = ArgValueCandidates::new(complete::branches))]
        range: Option<String>,
    },
}

fn main() -> ExitCode {
    // Hooks have a time limit, and building the full command-line parser
    // costs more than the rest of a hook. `prov hook` is parsed by hand.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [hook, adapter, event, rest @ ..] if hook == "hook" && !adapter.starts_with('-') && !event.starts_with('-') => {
            run_hook(adapter, event, rest)
        }
        _ => match parse_cli().command {
            Cmd::Hook { adapter, event, args } => run_hook(&adapter, &event, &args),
            Cmd::Init { agents, force, dry_run, json } => init::run(&init::Args { agents, force, dry_run, json }),
            Cmd::Doctor { deep, json } => doctor::run(deep, json),
            Cmd::Check { target, into, json } => check::run(target.as_deref(), into.as_deref(), json),
            Cmd::Start(a) => start::run(&start::Args {
                agent: a.agent,
                name: a.name,
                branch: a.branch,
                from: a.from,
                path: a.path,
                no_launch: a.no_launch,
                resume: a.resume,
                dry_run: a.dry_run,
                json: a.json,
                passthrough: a.agent_args,
            }),
            Cmd::Session { action } => match action {
                SessionCmd::List { json } => session::run(session::Action::List, json, false),
                SessionCmd::Show { session, json } => session::run(session::Action::Show(session), json, false),
                SessionCmd::Close { session, dry_run, json } => session::run(session::Action::Close(session), json, dry_run),
                SessionCmd::Drop { session, dry_run, json } => session::run(session::Action::Drop(session), json, dry_run),
            },
            Cmd::Finish { target, override_, force, dry_run, json } => {
                finish::run(&finish::Args { target, r#override: override_, force, dry_run, json })
            }
            Cmd::Discard { target, yes, dry_run, json } => discard::run(&discard::Args { target, yes, dry_run, json }),
            Cmd::Approve(a) => {
                let mode = match (a.merge, a.human, a.ai) {
                    (true, false, false) => Ok(approve::Mode::Merge),
                    (false, true, false) => Ok(approve::Mode::Human),
                    (false, false, true) => Ok(approve::Mode::Ai),
                    _ => Err(anyhow::anyhow!(prov_messages::cli::APPROVE_MODE)),
                };
                mode.and_then(|mode| {
                    approve::run(&approve::Args {
                        paths: a.paths,
                        mode,
                        agent: a.agent,
                        model: a.model,
                        dry_run: a.dry_run,
                        json: a.json,
                    })
                })
            }
            Cmd::Completions { shell, install, dry_run, json } => {
                completions::run(&completions::Args { shell, install, dry_run, json })
            }
            Cmd::Trace { action: TraceCmd::Export { range } } => trace::run(range.as_deref()),
            Cmd::Cp(a) => cp::run(&cp::Args {
                from: a.from,
                rev: a.rev,
                paths: a.paths,
                all: a.all,
                missing: a.missing,
                agent: a.agent,
                model: a.model,
                session: a.session,
                plan: a.plan,
                apply: a.apply,
                dry_run: a.dry_run,
                json: a.json,
            }),
            Cmd::CopyCheck { id } => copy_check::run(&id).map(|()| true),
            Cmd::ArchiveSession { session, generation } => archive::run(&session, generation).map(|()| true),
        },
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("{}", prov_messages::cli::error(format!("{e:#}")));
            // Never 2: Claude Code treats exit code 2 of a hook as a request to block.
            ExitCode::from(1)
        }
    }
}

/// The command line. With `COMPLETE=<shell>` set, this prints the shell's
/// completion script or the candidates for the shell instead, and exits.
fn parse_cli() -> Cli {
    clap_complete::CompleteEnv::with_factory(Cli::command).complete();
    Cli::parse()
}

fn run_hook(adapter: &str, event: &str, args: &[String]) -> anyhow::Result<bool> {
    if adapter == "git" {
        return git_hook::run(event, args);
    }
    let mut payload = String::new();
    std::io::stdin().read_to_string(&mut payload)?;
    agent_hook::run(adapter, event, &payload).map(|()| true)
}
