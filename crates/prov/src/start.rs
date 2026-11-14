//! `prov start <agent>`: a worktree on a new agent branch, and the agent in it.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use prov_messages as msg;
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
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).context(msg::start::CANNOT_READ_URANDOM)?;
    Ok(format!("{:02x}{:02x}", bytes[0], bytes[1]))
}

/// The git config key of the clone for the directory that holds the worktrees.
const WORKTREES_DIR_KEY: &str = "prov.worktreesDir";

/// The directory that holds the worktree: `--path`, then `prov.worktreesDir`
/// from the git config of the clone, then `../<repo>.worktrees/` from the
/// base directory. A relative `prov.worktreesDir` resolves against the base
/// directory.
fn worktrees_dir(ctx: &Ctx, path: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = path {
        return absolute(path);
    }
    let base = ctx.repo.base_dir();
    if let Some(dir) = ctx.repo.config_value(WORKTREES_DIR_KEY)?.filter(|d| !d.trim().is_empty()) {
        let dir = PathBuf::from(dir.trim());
        if dir.is_relative() && !dir.starts_with("~") {
            return absolute(&base.join(dir));
        }
        return absolute(&dir);
    }
    let name = base.file_name().and_then(|n| n.to_str()).context(msg::start::BASE_NO_NAME)?;
    Ok(base.parent().context(msg::start::BASE_NO_PARENT)?.join(format!("{name}.worktrees")))
}

/// The directory name of a worktree: the branch without `agent/`, with `-`
/// for `/`. `agent/claude/20261112-71fe` gives `claude-20261112-71fe`.
fn worktree_name(branch: &str) -> String {
    branch.strip_prefix("agent/").unwrap_or(branch).replace('/', "-")
}

pub fn run(args: &Args) -> Result<bool> {
    if let Some(var) = prov_adapters::agent_env_marker() {
        bail!(msg::start::agent_env(var));
    }
    let ctx = Ctx::here()?;
    let repo = &ctx.repo;
    let config = repo.config_here()?;
    if !config.agents.contains(&args.agent) {
        bail!(msg::start::agent_not_configured(&args.agent, config.agents.join(", ")));
    }
    let adapter =
        prov_adapters::by_agent_name(&args.agent).with_context(|| msg::archive::no_adapter(&args.agent))?;
    if let Some(name) = &args.name {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            bail!(msg::start::NAME_CHARACTERS);
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
            bail!(msg::start::not_agent_branch(&branch));
        }
        if repo.git_ok(["show-ref", "--verify", "-q", &format!("refs/heads/{branch}")])? {
            bail!(msg::start::branch_exists(&branch));
        }
        // The start point is the branch that is checked out: a feature branch too.
        let from = match &args.from {
            Some(from) => from.clone(),
            None => repo.branch.clone().unwrap_or_else(|| "HEAD".to_string()),
        };
        if repo.rev_parse(&from)?.is_none() {
            bail!(msg::check::no_branch(from));
        }
        let worktree = worktrees_dir(&ctx, args.path.as_deref())?.join(worktree_name(&branch));
        if worktree.exists() {
            bail!(msg::start::exists(worktree.display()));
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
        eprintln!("{}", msg::start::worktree(args.dry_run, report.created, &report.branch, report.from.as_ref()));
    }
    if args.resume.is_some() && !report.created {
        crate::procs::warn_if_busy(&report.worktree, msg::start::STOP_PROCESSES);
    }
    let Some(command) = &report.command else { return Ok(true) };
    if args.dry_run {
        return Ok(true);
    }

    // The agent replaces this process. Its commits get the agent identity and no signature.
    let identity = prov_adapters::identity(&config, &args.agent).context(msg::start::NO_IDENTITY)?;
    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..]).current_dir(&report.worktree);
    for (key, value) in prov_adapters::commit_env(&identity) {
        cmd.env(key, value);
    }
    let error = cmd.exec();
    Err(error).with_context(|| msg::start::cannot_start(&command[0]))
}

/// The absolute path, with `.` and `..` removed by text, not by following
/// links: `--path ../x` from `/r/project` gives `/r/x`. A leading `~` is the
/// home directory.
fn absolute(path: &Path) -> Result<PathBuf> {
    use std::path::Component;
    // The shell does not expand `~` in `--path=~/dir`, or in a quoted value.
    let home = || std::env::var_os("HOME").map(PathBuf::from).context(msg::completions::HOME_NOT_SET);
    let joined = match path.strip_prefix("~") {
        Ok(rest) => home()?.join(rest),
        Err(_) if path.is_absolute() => path.to_path_buf(),
        Err(_) => std::env::current_dir()?.join(path),
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

/// The worktree of a session to resume. If the worktree was removed, it is
/// created again on the session's branch.
fn resume(ctx: &Ctx, args: &Args, session: &str) -> Result<Report> {
    let repo = &ctx.repo;
    let record = ctx.store.get(session).ok().flatten();
    let branch = match (&record, &args.branch) {
        (Some(record), Some(branch)) if record.branch != *branch => {
            bail!(msg::start::session_on_other_branch(session, &record.branch, branch))
        }
        (Some(record), _) => record.branch.clone(),
        (None, Some(branch)) => branch.clone(),
        (None, None) => bail!(msg::start::session_no_record(session)),
    };
    if !is_agent_branch(&branch) {
        bail!(msg::start::not_agent_branch(&branch));
    }
    if !repo.git_ok(["show-ref", "--verify", "-q", &format!("refs/heads/{branch}")])? {
        bail!(msg::start::no_branch(&branch));
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
        (None, Some(record)) => record.worktree.clone(),
        (path, _) => worktrees_dir(ctx, path.as_deref())?.join(worktree_name(&branch)),
    };
    if worktree.exists() {
        bail!(msg::start::other_checkout(worktree.display(), &branch));
    }
    if !args.dry_run {
        repo.git(["worktree".as_ref(), "add".as_ref(), "-q".as_ref(), worktree.as_os_str(), branch.as_ref()])?;
    }
    Ok(Report { dry_run: args.dry_run, branch, worktree, created: true, from: None, command: None })
}

#[cfg(test)]
mod tests {
    use super::worktree_name;

    #[test]
    fn worktree_name_comes_from_the_branch() {
        assert_eq!(worktree_name("agent/claude/20261112-71fe"), "claude-20261112-71fe");
        assert_eq!(worktree_name("agent/claude/fix/parser"), "claude-fix-parser");
    }
}
