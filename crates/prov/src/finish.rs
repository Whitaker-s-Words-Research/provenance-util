//! `prov finish <session or branch>`: `git worktree remove`. The branch
//! stays: it is a record of the session. `prov` never deletes a branch.

use std::path::PathBuf;

use anyhow::{bail, Result};
use prov_core::State;
use serde::Serialize;

use crate::context::Ctx;
use crate::session;

pub struct Args {
    pub target: String,
    pub r#override: bool,
    pub force: bool,
    pub dry_run: bool,
    pub json: bool,
}

#[derive(Debug, Serialize)]
struct Report {
    dry_run: bool,
    worktree: PathBuf,
    branch: Option<String>,
    /// Sessions that `--override` closed.
    closed: Vec<String>,
    removed: bool,
    /// True if the branch is merged into the branch checked out here.
    #[serde(skip_serializing_if = "Option::is_none")]
    merged: Option<bool>,
    /// A command that deletes the branch. `prov` does not run it.
    #[serde(skip_serializing_if = "Option::is_none")]
    delete_command: Option<String>,
}

pub fn run(args: &Args) -> Result<bool> {
    let ctx = Ctx::here()?;
    let repo = &ctx.repo;
    let worktrees = repo.worktrees()?;

    // A branch name first: it is exact. Then a session ID or the start of one.
    let found = worktrees.iter().find(|w| w.branch.as_deref() == Some(args.target.as_str())).cloned();
    let (worktree, branch) = if let Some(w) = found { (w.path, w.branch) } else {
        let record = session::resolve(&ctx.store, &args.target).map_err(|e| {
            anyhow::anyhow!("`{}` is not a branch with a worktree, and not a session: {e}", args.target)
        })?;
        match worktrees.iter().find(|w| w.path == record.worktree) {
            Some(w) => (w.path.clone(), w.branch.clone()),
            None => bail!("the worktree {} of session {} does not exist", record.worktree.display(), record.id),
        }
    };
    if worktrees.first().is_some_and(|main| main.path == worktree) {
        bail!("{} is the main worktree. `prov finish` removes only a linked worktree.", worktree.display());
    }
    if repo.toplevel == worktree {
        bail!("this command runs inside {}. Run it from another worktree.", worktree.display());
    }

    let busy: Vec<_> = session::views(&ctx)?
        .into_iter()
        .filter(|v| v.record.worktree == worktree && matches!(v.record.state, State::Open | State::Closing))
        .collect();
    if !busy.is_empty() && !args.r#override {
        let list: Vec<String> = busy.iter().map(|v| format!("{} ({})", v.record.id, v.record.state.as_str())).collect();
        bail!(
            "refused: a session in {} is open or closing: {}. End the session or run `prov session close <id>`. \
             `--override` closes each open session first.",
            worktree.display(),
            list.join(", ")
        );
    }

    // Merged into HEAD here: `git branch -d` then deletes it, if the branch
    // has no upstream. Otherwise only `-D` does, and the work is gone.
    let merged = match &branch {
        Some(b) => Some(repo.is_ancestor(&format!("refs/heads/{b}"), "HEAD")?),
        None => None,
    };
    let delete_command =
        branch.as_ref().zip(merged).map(|(b, m)| format!("git branch {} {b}", if m { "-d" } else { "-D" }));
    let mut report = Report {
        dry_run: args.dry_run,
        worktree: worktree.clone(),
        branch,
        closed: Vec::new(),
        removed: false,
        merged,
        delete_command,
    };
    // The records stay: a later integration of the branch needs the transcript hash.
    for v in busy.iter().filter(|v| v.record.state == State::Open) {
        if !args.dry_run {
            let after = session::close(&ctx, &v.record.id, false)?;
            if after.state != State::Archived {
                bail!(
                    "session {} did not archive ({}). The worktree stays.",
                    after.id,
                    after.error.as_deref().unwrap_or(after.state.as_str())
                );
            }
        }
        report.closed.push(v.record.id.clone());
    }
    // R-SWEEP-4: a process that runs on writes into a worktree that is gone,
    // or into one that `prov start --resume` creates again.
    crate::procs::warn_if_busy(&worktree, "Stop them: the worktree is removed now.");
    if !args.dry_run {
        let mut git: Vec<&std::ffi::OsStr> = vec!["worktree".as_ref(), "remove".as_ref()];
        if args.force {
            git.push("--force".as_ref());
        }
        git.push(worktree.as_os_str());
        repo.git(git)?;
        report.removed = true;
        // The owner of a worktree that is gone is of no use.
        ctx.store.lock()?.delete_worktree(&worktree)?;
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for id in &report.closed {
            println!("{} session {id}", if args.dry_run { "will close" } else { "closed" });
        }
        println!(
            "{} the worktree {}. The branch {} stays.",
            if args.dry_run { "Dry run: will remove" } else { "Removed" },
            worktree.display(),
            report.branch.as_deref().unwrap_or("(detached)"),
        );
        if let (Some(command), Some(merged)) = (&report.delete_command, report.merged) {
            let here = repo.branch.as_deref().unwrap_or("HEAD");
            if merged {
                println!("It is merged into {here}. To delete it: {command}");
            } else {
                println!("It is not merged into {here}. To delete it anyway, with its commits: {command}");
            }
        }
    }
    Ok(true)
}
