//! `prov start <agent>`: a worktree on a new agent branch, and the agent in it.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use prov_core::pattern::is_agent_branch;
use serde::Serialize;

use crate::context::Ctx;

pub struct Args {
    pub agent: String,
    pub name: Option<String>,
    pub branch: Option<String>,
    pub from: Option<String>,
    pub path: Option<PathBuf>,
    pub no_launch: bool,
    pub resume: Option<String>,
    pub dry_run: bool,
    pub json: bool,
    pub passthrough: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Report {
    dry_run: bool,
    branch: String,
    worktree: PathBuf,
    /// True if this run creates the worktree.
    created: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    from: Option<String>,
    /// The command of the agent. `None` with `--no-launch`.
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<Vec<String>>,
}

/// The local date as `YYYYMMDD`.
fn local_date() -> String {
    let now = prov_core::now() as libc::time_t;
    // SAFETY: localtime_r writes only to `tm`, which is a valid, zeroed struct.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&raw const now, &raw mut tm) };
    format!("{:04}{:02}{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)
}

/// 4 random hex characters.
fn random_name() -> Result<String> {
    use std::io::Read;
    let mut bytes = [0u8; 2];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).context("cannot read /dev/urandom")?;
    Ok(format!("{:02x}{:02x}", bytes[0], bytes[1]))
}

/// `../<repo>.worktrees/`, from the base directory.
fn worktrees_dir(ctx: &Ctx) -> Result<PathBuf> {
    let base = ctx.repo.base_dir();
    let name = base.file_name().and_then(|n| n.to_str()).context("the base directory has no name")?;
    Ok(base.parent().context("the base directory has no parent")?.join(format!("{name}.worktrees")))
}

pub fn run(args: &Args) -> Result<bool> {
    let ctx = Ctx::here()?;
    let repo = &ctx.repo;
    let config = repo.config_here()?;
    if !config.agents.contains(&args.agent) {
        bail!("the agent `{}` is not in `agents` of .provenance.toml ({})", args.agent, config.agents.join(", "));
    }
    let adapter =
        prov_adapters::by_agent_name(&args.agent).with_context(|| format!("no adapter for the agent `{}`", args.agent))?;
    if let Some(name) = &args.name {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            bail!("--name takes letters, digits and `-` only");
        }
    }

    let mut report = if let Some(session) = &args.resume {
        resume(&ctx, args, session)?
    } else {
        let date = local_date();
        let name = match &args.name {
            Some(name) => name.clone(),
            None => random_name()?,
        };
        let branch = args.branch.clone().unwrap_or_else(|| format!("agent/{}/{date}-{name}", args.agent));
        if !is_agent_branch(&branch) {
            bail!("the branch `{branch}` does not match `agent/**`");
        }
        if repo.git_ok(["show-ref", "--verify", "-q", &format!("refs/heads/{branch}")])? {
            bail!("the branch `{branch}` exists. Give another --name, or continue its session with --resume.");
        }
        // The start point is the branch that is checked out: a feature branch too.
        let from = match &args.from {
            Some(from) => from.clone(),
            None => repo.branch.clone().unwrap_or_else(|| "HEAD".to_string()),
        };
        if repo.rev_parse(&from)?.is_none() {
            bail!("no branch or commit `{from}`");
        }
        let worktree = match &args.path {
            Some(path) => absolute(path)?,
            None => worktrees_dir(&ctx)?.join(format!("{}-{date}-{name}", args.agent)),
        };
        if worktree.exists() {
            bail!("{} exists", worktree.display());
        }
        if !args.dry_run {
            repo.git([
                "worktree".as_ref(),
                "add".as_ref(),
                "-q".as_ref(),
                "-b".as_ref(),
                branch.as_ref(),
                worktree.as_os_str(),
                from.as_ref(),
            ])?;
        }
        Report { dry_run: args.dry_run, branch, worktree, created: true, from: Some(from), command: None }
    };

    if !args.no_launch {
        let mut command = match config.agent_section(&args.agent).and_then(|s| s.command.clone()) {
            Some(command) if !command.is_empty() => command,
            _ => adapter.default_command(),
        };
        if let Some(session) = &args.resume {
            command.extend(adapter.resume_args(session));
        }
        command.extend(args.passthrough.iter().cloned());
        report.command = Some(command);
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        // The path alone on standard output: `cd "$(prov start claude --no-launch)"`.
        println!("{}", report.worktree.display());
        eprintln!(
            "prov: {}worktree {} on the branch {}{}",
            if args.dry_run { "dry run: " } else { "" },
            if report.created { "created" } else { "exists" },
            report.branch,
            report.from.as_ref().map_or_default(|f| format!(", from {f}")),
        );
    }
    if args.resume.is_some() && !report.created {
        crate::procs::warn_if_busy(&report.worktree, "Stop them before the session goes on, or its sweep takes their changes.");
    }
    let Some(command) = &report.command else { return Ok(true) };
    if args.dry_run {
        return Ok(true);
    }

    // The agent replaces this process. Its commits get the agent identity and no signature.
    let identity = prov_adapters::identity(&config, &args.agent).context("the agent has no commit identity")?;
    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..]).current_dir(&report.worktree);
    for (key, value) in prov_adapters::commit_env(&identity) {
        cmd.env(key, value);
    }
    let error = cmd.exec();
    Err(error).with_context(|| format!("cannot start `{}`", command[0]))
}

fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir()?.join(path) })
}

/// The worktree of a session to resume. If the worktree was removed, it is
/// created again on the session's branch.
fn resume(ctx: &Ctx, args: &Args, session: &str) -> Result<Report> {
    let repo = &ctx.repo;
    let record = ctx.store.get(session).ok().flatten();
    let branch = match (&record, &args.branch) {
        (Some(record), Some(branch)) if record.branch != *branch => {
            bail!("the session {session} is on the branch `{}`, not `{branch}`", record.branch)
        }
        (Some(record), _) => record.branch.clone(),
        (None, Some(branch)) => branch.clone(),
        (None, None) => bail!("the session {session} has no record (it is integrated, or it ran in another clone). Also give --branch."),
    };
    if !is_agent_branch(&branch) {
        bail!("the branch `{branch}` does not match `agent/**`");
    }
    if !repo.git_ok(["show-ref", "--verify", "-q", &format!("refs/heads/{branch}")])? {
        bail!("no branch `{branch}`");
    }
    // A worktree that has the branch checked out now.
    if let Some(existing) = repo.worktrees()?.into_iter().find(|w| w.branch.as_deref() == Some(branch.as_str())) {
        if existing.path.is_dir() {
            return Ok(Report { dry_run: args.dry_run, branch, worktree: existing.path, created: false, from: None, command: None });
        }
        // The directory is gone but git still lists the worktree.
        if !args.dry_run {
            repo.git(["worktree", "prune"])?;
        }
    }
    let worktree = match (&args.path, &record) {
        (Some(path), _) => absolute(path)?,
        (None, Some(record)) => record.worktree.clone(),
        (None, None) => {
            let name = branch.strip_prefix("agent/").unwrap_or(&branch).replace('/', "-");
            worktrees_dir(ctx)?.join(name)
        }
    };
    if worktree.exists() {
        bail!("{} exists, and it does not have the branch `{branch}` checked out", worktree.display());
    }
    if !args.dry_run {
        repo.git(["worktree".as_ref(), "add".as_ref(), "-q".as_ref(), worktree.as_os_str(), branch.as_ref()])?;
    }
    Ok(Report { dry_run: args.dry_run, branch, worktree, created: true, from: None, command: None })
}
