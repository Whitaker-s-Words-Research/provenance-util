//! `prov discard <session or branch>`: deletes the work of an agent branch
//! that will not be integrated. The worktree, the branch, the session
//! records and the transcript archives. See SPEC.md, "Discarding".

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use prov_core::pattern::is_agent_branch;
use prov_core::trailer::{self, SessionRef};
use prov_core::{now, State};
use prov_git::Repo;
use serde::Serialize;

use crate::context::{say, Ctx};
use crate::session;

pub struct Args {
    pub target: String,
    pub yes: bool,
    pub dry_run: bool,
    pub json: bool,
}

/// R-DISC-1: when a ref changes between the check and the lock, the check
/// runs again, at most this many times.
const ATTEMPTS: usize = 3;

#[derive(Debug, Clone, Serialize)]
struct Report {
    dry_run: bool,
    /// True when everything in the report is deleted.
    deleted: bool,
    branch: String,
    /// False when the branch does not exist, for example after an interrupted discard.
    branch_exists: bool,
    /// The commits that only this branch reaches. They are lost.
    own_commits: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree: Option<PathBuf>,
    /// Files of the worktree that are not committed. They are lost.
    changed_files: usize,
    sessions: Vec<SessionItem>,
    archives: Vec<PathBuf>,
    /// Remote branches of the same name. `prov discard` keeps them.
    remote_branches: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SessionItem {
    agent: String,
    id: String,
    /// The state of the record, or `None` for a session that a trailer names and that has no record.
    state: Option<String>,
}

/// Everything that step 1 finds.
struct Plan {
    report: Report,
    /// Sessions with a record that names the branch.
    records: Vec<SessionRef>,
    /// Every session of the discard.
    sessions: BTreeSet<SessionRef>,
    own: BTreeSet<String>,
    refs: String,
}

pub fn run(args: &Args) -> Result<bool> {
    let ctx = Ctx::here()?;
    if let Some(var) = prov_adapters::agent_env_marker() {
        bail!("refused: only a human can run `prov discard`. {var} is set: an agent started this command.");
    }
    if ctx.on_agent_branch() {
        bail!("refused: `prov discard` does not run in an agent worktree. Run it from a human worktree.");
    }
    let branch = find_branch(&ctx, &args.target)?;

    for attempt in 1..=ATTEMPTS {
        let plan = collect(&ctx, &branch)?;
        if args.dry_run || !args.yes {
            print(&plan.report, args);
            return Ok(true);
        }
        if !args.json {
            print(&plan.report, args);
        }
        if !delete_under_lock(&ctx, &plan)? {
            if attempt == ATTEMPTS {
                bail!("refused: refs of this repository changed {ATTEMPTS} times during the check. Run it again when they stop changing.");
            }
            say("a ref changed during the check. Checking again.");
            continue;
        }
        // Step 3: the worktree.
        if let Some(worktree) = &plan.report.worktree {
            crate::procs::warn_if_busy(worktree, "Stop them: the worktree is removed now.");
            let mut git: Vec<&std::ffi::OsStr> = vec!["worktree".as_ref(), "remove".as_ref(), "--force".as_ref()];
            git.push(worktree.as_os_str());
            ctx.repo.git(git)?;
            ctx.store.lock()?.delete_worktree(worktree)?;
        }
        // Step 4: the branch.
        if plan.report.branch_exists {
            ctx.repo.git(["branch", "-D", &branch])?;
        }
        let mut report = plan.report;
        report.deleted = true;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            println!("Discarded the branch {branch}.");
            for remote in &report.remote_branches {
                if let Some((name, b)) = remote.strip_prefix("refs/remotes/").and_then(|r| r.split_once('/')) {
                    println!("The remote branch {remote} stays. To delete it: git push {name} --delete {b}");
                }
            }
        }
        return Ok(true);
    }
    unreachable!("the loop returns or fails")
}

/// The branch: a local agent branch, or the branch of a session record.
fn find_branch(ctx: &Ctx, target: &str) -> Result<String> {
    if is_agent_branch(target) && ctx.repo.rev_parse(&format!("refs/heads/{target}"))?.is_some() {
        return Ok(target.to_string());
    }
    if let Ok(record) = session::resolve(&ctx.store, target) {
        if !is_agent_branch(&record.branch) {
            bail!("session {} is on {}, which is not an agent branch", record.id, record.branch);
        }
        return Ok(record.branch);
    }
    // An interrupted discard leaves records or a worktree for a branch that is gone.
    if is_agent_branch(target)
        && (ctx.store.sessions()?.iter().any(|r| r.branch == target)
            || ctx.repo.worktrees()?.iter().any(|w| w.branch.as_deref() == Some(target)))
    {
        return Ok(target.to_string());
    }
    bail!("`{target}` is not an agent branch, and not a session. See: git branch --list 'agent/*', prov session list")
}

/// The arguments that select every ref except the branch, its remote-tracking
/// branches and the notes. `--single-worktree`: without it, `--all` includes
/// the `HEAD` of the branch's own worktree.
fn other_refs(branch: &str) -> Vec<String> {
    vec![
        "--single-worktree".into(),
        format!("--exclude=refs/heads/{branch}"),
        format!("--exclude=refs/remotes/*/{branch}"),
        "--exclude=refs/notes/*".into(),
        "--all".into(),
    ]
}

/// Commits that `tip` reaches and no other ref reaches.
fn own_commits(repo: &Repo, tip: &str, branch: &str) -> Result<BTreeSet<String>> {
    let mut args = vec!["rev-list".to_string(), tip.to_string(), "--not".into()];
    args.extend(other_refs(branch));
    Ok(repo.git(args)?.lines().map(String::from).collect())
}

/// The value of every ref except the notes. Notes change with every commit,
/// and no rule here depends on them.
fn ref_values(repo: &Repo) -> Result<String> {
    let text = repo.git(["for-each-ref", "--format=%(objectname) %(refname)"])?;
    Ok(text.lines().filter(|l| !l.contains(" refs/notes/")).collect::<Vec<_>>().join("\n"))
}

/// Step 1, without the lock.
fn collect(ctx: &Ctx, branch: &str) -> Result<Plan> {
    let repo = &ctx.repo;
    let refs = ref_values(repo)?;
    let tip = repo.rev_parse(&format!("refs/heads/{branch}"))?;
    let own = match &tip {
        Some(tip) => own_commits(repo, tip, branch)?,
        None => BTreeSet::new(),
    };

    let worktree = repo.worktrees()?.into_iter().find(|w| w.branch.as_deref() == Some(branch));
    if let Some(w) = &worktree {
        if w.path == repo.toplevel {
            bail!("refused: the branch {branch} is checked out here. Run `prov discard` from another worktree.");
        }
        if repo.worktrees()?.first().is_some_and(|main| main.path == w.path) {
            bail!("refused: the branch {branch} is checked out in the main worktree {}. Check out another branch there first.", w.path.display());
        }
    }

    // The sessions: records that name the branch, and trailers of its own commits.
    let views = session::views(ctx)?;
    let mut sessions: BTreeSet<SessionRef> = BTreeSet::new();
    let mut records = Vec::new();
    let mut items = Vec::new();
    for v in views.iter().filter(|v| v.record.branch == branch) {
        let r = &v.record;
        let s = SessionRef::new(&r.agent, &r.id);
        sessions.insert(s.clone());
        records.push(s);
        items.push(SessionItem { agent: r.agent.clone(), id: r.id.clone(), state: Some(state_text(r.state, v.stale)) });
    }
    if let Some(tip) = &tip {
        let mut args = vec![tip.clone(), "--not".into()];
        args.extend(other_refs(branch));
        for commit in repo.commits(args)? {
            for s in trailer::sessions(&commit.trailers) {
                if sessions.contains(&s) {
                    continue;
                }
                if let Some(v) = views.iter().find(|v| v.record.id == s.id && v.record.agent == s.agent) {
                    bail!(
                        "refused: commit {} names session {s}, and its record is for the branch {}, not {branch}. \
                         Discard that branch, or drop the record (prov session drop {}).",
                        commit.short(),
                        v.record.branch,
                        v.record.id
                    );
                }
                items.push(SessionItem { agent: s.agent.clone(), id: s.id.clone(), state: None });
                sessions.insert(s);
            }
        }
    }

    // A session that is Open (also stale) or Closing.
    let busy: Vec<String> = views
        .iter()
        .filter(|v| v.record.branch == branch && matches!(v.record.state, State::Open | State::Closing))
        .map(|v| format!("{} ({})", v.record.id, state_text(v.record.state, v.stale)))
        .collect();
    if !busy.is_empty() {
        bail!("refused: a session of {branch} is open or closing: {}. Close it first: prov session close <id>", busy.join(", "));
    }

    // Other refs that name a session, or copy from an own commit.
    let mut args: Vec<String> = vec!["--grep=^Prov-Session:".into(), "--grep=^Copied-From:".into()];
    args.extend(other_refs(branch));
    for commit in repo.commits(args)? {
        if let Some(s) = trailer::sessions(&commit.trailers).into_iter().find(|s| sessions.contains(s)) {
            bail!(
                "refused: commit {} on another ref names session {s}. That ref needs the session's record or archive. \
                 To remove only the worktree: prov finish {branch}",
                commit.short()
            );
        }
        for t in commit.trailers.iter().filter(|t| t.key == trailer::COPIED_FROM) {
            if let Some(source) = trailer::copied_from_commit(&t.value).filter(|c| own.contains(*c)) {
                bail!(
                    "refused: commit {} copied a file from commit {source} of {branch} (Copied-From). \
                     The transcripts of that copy are found through this branch.",
                    commit.short()
                );
            }
        }
    }
    for copy in ctx.store.copies()? {
        if own.contains(&copy.source_commit) {
            bail!(
                "refused: the copy {} has its source in commit {} of {branch}. Commit or delete the copied files first.",
                copy.id, copy.source_commit
            );
        }
    }
    merge_check(ctx, branch, &own)?;

    let config = repo.config_here()?;
    let mut archives = Vec::new();
    for s in &sessions {
        archives.extend(archives_of(&repo.base_dir().join(&config.logs_dir).join(&s.agent), &s.id)?);
    }
    archives.sort();
    let changed_files = match &worktree {
        Some(w) => Repo::discover(&w.path)?.map_or(Ok(0), |r| r.changed_files().map(|f| f.len()))?,
        None => 0,
    };
    let remote_branches = repo
        .git(["for-each-ref", "--format=%(refname)", "refs/remotes/"])?
        .lines()
        .filter(|r| r.ends_with(&format!("/{branch}")))
        .map(String::from)
        .collect();

    Ok(Plan {
        report: Report {
            dry_run: false,
            deleted: false,
            branch: branch.to_string(),
            branch_exists: tip.is_some(),
            own_commits: own.len(),
            worktree: worktree.map(|w| w.path),
            changed_files,
            sessions: items,
            archives,
            remote_branches,
        },
        records,
        sessions,
        own,
        refs,
    })
}

fn state_text(state: State, is_stale: bool) -> String {
    if is_stale { format!("{}, stale", state.as_str()) } else { state.as_str().to_string() }
}

/// Refuses while a merge is in progress in any worktree whose `MERGE_HEAD`
/// reaches one of the branch's own commits. Reason: its gate may have passed,
/// and its commit will name the archives (R-DISC-1).
fn merge_check(ctx: &Ctx, branch: &str, own: &BTreeSet<String>) -> Result<()> {
    if own.is_empty() {
        return Ok(());
    }
    for w in ctx.repo.worktrees()? {
        let Some(repo) = Repo::discover(&w.path)? else { continue };
        for head in repo.merge_heads()? {
            if own.contains(&head) || own_commits(&ctx.repo, &head, branch)?.iter().any(|c| own.contains(c)) {
                bail!(
                    "refused: a merge of {branch} is in progress in {}. Finish it or run `git merge --abort` there first.",
                    w.path.display()
                );
            }
        }
    }
    Ok(())
}

/// The archive files of a session: `<session>-<12 hex>.tar.gz`.
fn archives_of(dir: &Path, id: &str) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", dir.display())),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let is_archive = name
            .strip_prefix(id)
            .and_then(|rest| rest.strip_prefix('-'))
            .and_then(|rest| rest.strip_suffix(".tar.gz"))
            .is_some_and(|hash| hash.len() == 12 && hash.bytes().all(|b| b.is_ascii_hexdigit()));
        if is_archive {
            out.push(entry.path());
        }
    }
    Ok(out)
}

/// Step 2 (R-DISC-1). Returns false, with nothing deleted, if a ref changed
/// since step 1.
fn delete_under_lock(ctx: &Ctx, plan: &Plan) -> Result<bool> {
    let lock = ctx.store.lock()?;
    if ref_values(&ctx.repo)? != plan.refs {
        return Ok(false);
    }
    let now = now();
    for record in lock.sessions()?.into_iter().map(|r| r.at(now)) {
        let named = plan.sessions.contains(&SessionRef::new(&record.agent, &record.id));
        if (record.branch == plan.report.branch || named) && matches!(record.state, State::Open | State::Closing) {
            bail!("refused: session {} became {} during the check. Nothing was deleted.", record.id, record.state.as_str());
        }
        if named && record.branch != plan.report.branch {
            bail!("refused: session {} now has a record for the branch {}. Nothing was deleted.", record.id, record.branch);
        }
    }
    merge_check(ctx, &plan.report.branch, &plan.own)?;
    for file in &plan.report.archives {
        match std::fs::remove_file(file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("cannot delete {}", file.display())),
        }
    }
    for record in lock.sessions()?.into_iter().filter(|r| r.branch == plan.report.branch) {
        lock.delete(&record.id)?;
    }
    for s in &plan.records {
        lock.delete(&s.id)?;
    }
    Ok(true)
}

fn print(report: &Report, args: &Args) {
    let report = Report { dry_run: args.dry_run, ..report.clone() };
    if args.json {
        match serde_json::to_string_pretty(&report) {
            Ok(text) => println!("{text}"),
            Err(e) => say(format!("error: {e}")),
        }
        return;
    }
    let will = if args.dry_run || !args.yes { "Will delete" } else { "Deleting" };
    println!("{will}:");
    if let Some(w) = &report.worktree {
        println!("  the worktree {}, with {} changed files that are not committed", w.display(), report.changed_files);
    }
    if report.branch_exists {
        println!("  the branch {}, with {} commits that no other ref has", report.branch, report.own_commits);
    }
    for s in &report.sessions {
        match &s.state {
            Some(state) => println!("  the record of session {}:{} ({state})", s.agent, s.id),
            None => println!("  session {}:{} (no record)", s.agent, s.id),
        }
    }
    for a in &report.archives {
        println!("  the archive {}", a.display());
    }
    println!("It keeps the agent's own transcripts and the git-ai notes.");
    for remote in &report.remote_branches {
        println!("It keeps the remote branch {remote}.");
    }
    if args.dry_run {
        println!("Dry run: nothing was deleted.");
    } else if !args.yes {
        println!("Nothing was deleted. To delete it: prov discard {} --yes", report.branch);
    }
}
