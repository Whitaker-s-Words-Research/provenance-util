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
use prov_messages as msg;
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
        bail!(msg::discard::agent_env(var));
    }
    if ctx.on_agent_branch() {
        bail!(msg::discard::AGENT_WORKTREE);
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
        if attempt == 1 {
            if let Some(worktree) = &plan.report.worktree {
                crate::procs::warn_if_busy(worktree, msg::discard::STOP_PROCESSES);
            }
        }
        if !delete_under_lock(&ctx, &plan)? {
            if attempt == ATTEMPTS {
                bail!(msg::discard::refs_kept_changing(ATTEMPTS));
            }
            say(msg::discard::REF_CHANGED);
            continue;
        }
        // Step 3: the branch.
        if plan.report.branch_exists {
            ctx.repo.git(["branch", "-D", &branch])?;
        }
        let mut report = plan.report;
        report.deleted = true;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            println!("{}", msg::discard::discarded(&branch));
            for remote in &report.remote_branches {
                if let Some((name, b)) = remote.strip_prefix("refs/remotes/").and_then(|r| r.split_once('/')) {
                    println!("{}", msg::discard::remote_stays(remote, name, b));
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
            bail!(msg::discard::session_not_agent_branch(&record.id, &record.branch));
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
    bail!(msg::discard::not_found(target))
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
            bail!(msg::discard::checked_out_here(branch));
        }
        if repo.worktrees()?.first().is_some_and(|main| main.path == w.path) {
            bail!(msg::discard::checked_out_in_main(branch, w.path.display()));
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
                    bail!(msg::discard::session_of_other_branch(commit.short(), &s, &v.record.branch, branch, &v.record.id));
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
        .map(|v| msg::discard::busy_session(&v.record.id, state_text(v.record.state, v.stale)))
        .collect();
    if !busy.is_empty() {
        bail!(msg::discard::busy(branch, busy.join(", ")));
    }

    // Other refs that name a session, or copy from an own commit.
    let mut args: Vec<String> = vec!["--grep=^Prov-Session:".into(), "--grep=^Copied-From:".into()];
    args.extend(other_refs(branch));
    for commit in repo.commits(args)? {
        if let Some(s) = trailer::sessions(&commit.trailers).into_iter().find(|s| sessions.contains(s)) {
            bail!(msg::discard::named_on_other_ref(commit.short(), &s, branch));
        }
        for t in commit.trailers.iter().filter(|t| t.key == trailer::COPIED_FROM) {
            if let Some(source) = trailer::copied_from_commit(&t.value).filter(|c| own.contains(*c)) {
                bail!(msg::discard::copied_from_own(commit.short(), source, branch));
            }
        }
    }
    for copy in ctx.store.copies()? {
        if own.contains(&copy.source_commit) {
            bail!(msg::discard::copy_in_progress(&copy.id, &copy.source_commit, branch));
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
    if is_stale { format!("{}{}", state.as_str(), msg::session::STALE) } else { state.as_str().to_string() }
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
                bail!(msg::discard::merge_in_progress(branch, w.path.display()));
            }
        }
    }
    Ok(())
}

/// The archive files of a session: `<session>-<12 hex>.tar.gz`, and the
/// temporary files `<session>-<12 hex>.tar.tmp<pid>` of a killed archive step.
fn archives_of(dir: &Path, id: &str) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| msg::fs::cannot_read(dir.display())),
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
        // A temporary file of a killed archive step holds part of the transcript.
        if is_archive || crate::archive::temp_archive_pid(name, id).is_some() {
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
            bail!(msg::discard::session_became(&record.id, record.state.as_str()));
        }
        if named && record.branch != plan.report.branch {
            bail!(msg::discard::session_moved(&record.id, &record.branch));
        }
    }
    merge_check(ctx, &plan.report.branch, &plan.own)?;
    // The worktree first, still under the lock: an agent event that waits
    // for the lock then finds no worktree and makes no record. Removed after
    // the records, a resume could make a new Open record for it.
    if let Some(worktree) = &plan.report.worktree {
        if worktree.exists() {
            let mut git: Vec<&std::ffi::OsStr> = vec!["worktree".as_ref(), "remove".as_ref(), "--force".as_ref()];
            git.push(worktree.as_os_str());
            ctx.repo.git(git)?;
        }
        lock.delete_worktree(worktree)?;
    }
    for file in &plan.report.archives {
        match std::fs::remove_file(file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| msg::fs::cannot_delete(file.display())),
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
            Err(e) => say(msg::discard::json_error(e)),
        }
        return;
    }
    println!("{}", if args.dry_run || !args.yes { msg::discard::WILL_DELETE } else { msg::discard::DELETING });
    if let Some(w) = &report.worktree {
        println!("{}", msg::discard::item_worktree(w.display(), report.changed_files));
    }
    if report.branch_exists {
        println!("{}", msg::discard::item_branch(&report.branch, report.own_commits));
    }
    for s in &report.sessions {
        match &s.state {
            Some(state) => println!("{}", msg::discard::item_record(&s.agent, &s.id, state)),
            None => println!("{}", msg::discard::item_session(&s.agent, &s.id)),
        }
    }
    for a in &report.archives {
        println!("{}", msg::discard::item_archive(a.display()));
    }
    println!("{}", msg::discard::KEEPS);
    for remote in &report.remote_branches {
        println!("{}", msg::discard::keeps_remote(remote));
    }
    if args.dry_run {
        println!("{}", msg::discard::DRY_RUN);
    } else if !args.yes {
        println!("{}", msg::discard::not_deleted(&report.branch));
    }
}
