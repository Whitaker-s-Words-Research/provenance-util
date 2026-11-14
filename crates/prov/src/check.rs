//! `prov check [<agent branch> | <base>..<tip>]`: the rules of the merge
//! gate without a merge. It changes nothing.
//!
//! Every branch that does not match `agent/**` is a human branch. The forms
//! with no argument and with an agent branch compare against the branch that
//! the merge will go into: `--into`, or the branch that is checked out.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context, Result};
use prov_messages as msg;
use prov_core::pattern::is_agent_branch;
use prov_core::trailer::{self, Choice};
use prov_core::Config;
use prov_git::CommitInfo;
use prov_gitai::Author;
use serde::Serialize;

use crate::context::Ctx;
use crate::gate::{self, Item};

#[derive(Debug, Default, Serialize)]
struct Report {
    ok: bool,
    /// What failed. Empty if `ok`.
    failures: Vec<Failure>,
    /// Form with no argument: each agent branch that is not integrated.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    branches: Vec<BranchReport>,
    /// Form with an agent branch: the items that the gate will show.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    items: Vec<Item>,
    /// Forms with no argument and with an agent branch: the branch that the
    /// merge will go into.
    #[serde(skip_serializing_if = "Option::is_none")]
    into: Option<String>,
    /// Form with a range: the number of commits checked.
    #[serde(skip_serializing_if = "Option::is_none")]
    commits_checked: Option<usize>,
    /// What looks wrong but does not fail the check.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<Failure>,
}

#[derive(Debug, Serialize)]
struct Failure {
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    message: String,
}

#[derive(Debug, Serialize)]
struct BranchReport {
    branch: String,
    commits: Vec<String>,
    sessions: Vec<SessionStatus>,
    doubtful_lines: u32,
    protected_paths: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SessionStatus {
    session: String,
    /// `open`, `closing`, `archived`, `failed`, or `no record`.
    state: String,
}

impl Report {
    fn fail(&mut self, commit: Option<&CommitInfo>, message: impl Into<String>) {
        self.failures.push(Failure { commit: commit.map(|c| c.sha.clone()), message: message.into() });
    }

    fn warn(&mut self, commit: Option<&CommitInfo>, message: impl Into<String>) {
        self.warnings.push(Failure { commit: commit.map(|c| c.sha.clone()), message: message.into() });
    }
}

/// What the range check needs to find a commit with no note.
struct Notes {
    /// The commits with a git-ai note.
    with_note: BTreeSet<String>,
    /// The time of the first commit of `refs/notes/ai`. Older commits were
    /// made before git-ai was in use.
    since: u64,
    /// The commit time of each commit of the range.
    times: BTreeMap<String, u64>,
    /// `known_gaps` of the config at the tip.
    known_gaps: Vec<String>,
}

impl Notes {
    /// `None` if the clone has no `refs/notes/ai`.
    fn read(repo: &prov_git::Repo, base: &str, tip: &str, known_gaps: &[String]) -> Result<Option<Notes>> {
        if repo.rev_parse("refs/notes/ai")?.is_none() {
            return Ok(None);
        }
        let since = repo
            .git(["log", "--max-parents=0", "--format=%ct", "refs/notes/ai"])?
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .min()
            .unwrap_or(0);
        let with_note = repo
            .git(["notes", "--ref=ai", "list"])?
            .lines()
            .filter_map(|l| l.split_whitespace().nth(1).map(str::to_string))
            .collect();
        let times = repo
            .git(["log", "--format=%H %ct", &format!("{base}..{tip}")])?
            .lines()
            .filter_map(|l| {
                let (sha, ct) = l.split_once(' ')?;
                Some((sha.to_string(), ct.trim().parse().ok()?))
            })
            .collect();
        let known_gaps = known_gaps.iter().map(|g| g.to_ascii_lowercase()).collect();
        Ok(Some(Notes { with_note, since, times, known_gaps }))
    }

    /// True if the commit should have a note and has none.
    fn missing(&self, sha: &str) -> bool {
        !self.with_note.contains(sha)
            && self.times.get(sha).is_some_and(|t| *t >= self.since)
            && !self.known_gaps.iter().any(|g| sha.starts_with(g.as_str()))
    }
}

/// Returns false if one check fails.
pub fn run(target: Option<&str>, into: Option<&str>, json: bool) -> Result<bool> {
    let ctx = Ctx::here()?;
    let mut report = Report::default();
    match target {
        Some(t) if t.contains("...") => bail!(msg::check::RANGE_SYNTAX),
        Some(t) if t.contains("..") => {
            if into.is_some() {
                bail!(msg::check::INTO_WITH_RANGE);
            }
            let (base, tip) = t.split_once("..").unwrap_or_default();
            check_range(&ctx, base, tip, &mut report)?;
        }
        Some(t) => {
            let into = merge_target(&ctx, into)?;
            check_branch(&ctx, t, &into, &mut report)?;
            report.into = Some(into);
        }
        None => {
            let into = merge_target(&ctx, into)?;
            list_branches(&ctx, &into, &mut report)?;
            report.into = Some(into);
        }
    }
    report.ok = report.failures.is_empty();
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print(&report, target);
    }
    Ok(report.ok)
}

/// The branch that a merge will go into: `--into`, or the branch that is
/// checked out. It is a human branch, which is any branch that does not match
/// `agent/**`.
fn merge_target(ctx: &Ctx, into: Option<&str>) -> Result<String> {
    let target = match (into, &ctx.repo.branch) {
        (Some(branch), _) => branch.to_string(),
        (None, Some(branch)) if !is_agent_branch(branch) => branch.clone(),
        (None, Some(branch)) => bail!(msg::check::on_agent_branch(branch)),
        (None, None) => bail!(msg::check::DETACHED),
    };
    if is_agent_branch(&target) {
        bail!(msg::check::into_agent_branch(&target));
    }
    if ctx.repo.rev_parse(&target)?.is_none() {
        bail!(msg::check::no_branch(&target));
    }
    Ok(target)
}

fn print(report: &Report, target: Option<&str>) {
    for b in &report.branches {
        println!("{}", b.branch);
        println!("{}", msg::check::commits(b.commits.len()));
        for c in &b.commits {
            println!("{}", msg::check::commit_line(c));
        }
        let sessions: Vec<String> = b.sessions.iter().map(|s| msg::check::session_state(&s.session, &s.state)).collect();
        println!("{}", msg::check::sessions(if sessions.is_empty() { msg::check::NO_SESSIONS.into() } else { sessions.join(", ") }));
        println!("{}", msg::check::doubtful_lines(b.doubtful_lines));
        if !b.protected_paths.is_empty() {
            println!("{}", msg::check::protected_paths(b.protected_paths.join(", ")));
        }
    }
    if target.is_none() && report.branches.is_empty() && report.failures.is_empty() {
        println!("{}", msg::check::nothing_waits(report.into.as_deref().unwrap_or("HEAD")));
    }
    for w in &report.warnings {
        match &w.commit {
            Some(c) => println!("{}", msg::check::warn_commit(&c[..c.len().min(10)], &w.message)),
            None => println!("{}", msg::check::warn(&w.message)),
        }
    }
    for f in &report.failures {
        match &f.commit {
            Some(c) => println!("{}", msg::check::fail_commit(&c[..c.len().min(10)], &f.message)),
            None => println!("{}", msg::check::fail(&f.message)),
        }
    }
    if let Some(n) = report.commits_checked {
        println!("{}", msg::check::summary(n, report.failures.len()));
    } else if target.is_some() && report.ok {
        println!("{}", msg::check::ok(report.into.as_deref().unwrap_or("HEAD")));
    }
}

fn session_state(ctx: &Ctx, session: &trailer::SessionRef) -> Result<String> {
    Ok(match gate::record_of(ctx, session)? {
        Some(r) => r.state.as_str().to_string(),
        None => msg::check::NO_RECORD.to_string(),
    })
}

/// `prov check`: the agent branches that are not merged into `human`.
fn list_branches(ctx: &Ctx, human: &str, report: &mut Report) -> Result<()> {
    let repo = &ctx.repo;
    let config = match repo.config_at(human)? {
        Some(config) => config,
        None => repo.config_here()?,
    };
    // git-ai writes a note some seconds after a commit. Without the note,
    // every line of a new commit would count as doubtful.
    // As in the gate: a failed await is not fatal. A commit with no note
    // gives items that need an approval.
    if let Err(e) = ctx.gitai.await_notes(&repo.toplevel) {
        crate::context::say(msg::gate::await_failed(format!("{e:#}")));
    }
    let refs = repo.git(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])?;
    for branch in refs.lines().filter(|b| is_agent_branch(b)) {
        if repo.is_ancestor(branch, human)? {
            continue;
        }
        let mut analysis = gate::analyze(repo, &ctx.gitai, &config, human, branch)?;
        gate::drop_sessions_without_record(ctx, &mut analysis)?;
        report.branches.push(BranchReport {
            branch: branch.to_string(),
            commits: analysis.commits.iter().map(|c| format!("{} {}", c.short(), c.subject)).collect(),
            sessions: analysis
                .sessions
                .iter()
                .map(|s| Ok(SessionStatus { session: s.to_string(), state: session_state(ctx, s)? }))
                .collect::<Result<_>>()?,
            doubtful_lines: analysis.doubtful_lines(),
            protected_paths: analysis
                .items
                .iter()
                .filter(|i| i.reason == gate::Reason::Protected)
                .map(|i| i.path.clone())
                .collect(),
        });
    }
    Ok(())
}

/// `prov check <agent branch>`: steps 2 to 4 of the gate for a merge of that
/// branch into `human`.
fn check_branch(ctx: &Ctx, branch: &str, human: &str, report: &mut Report) -> Result<()> {
    let repo = &ctx.repo;
    if repo.rev_parse(branch)?.is_none() {
        bail!(msg::check::no_branch(branch));
    }
    let Some(config) = repo.config_at(human).with_context(|| msg::check::config_of(human))? else {
        report.fail(None, msg::check::branch_no_config(human));
        return Ok(());
    };
    if let Err(e) = config.check_version(prov_core::VERSION) {
        report.fail(None, e.to_string());
    }
    // As in the gate: a failed await is not fatal. A commit with no note
    // gives items that need an approval.
    if let Err(e) = ctx.gitai.await_notes(&repo.toplevel) {
        crate::context::say(msg::gate::await_failed(format!("{e:#}")));
    }
    let mut analysis = gate::analyze(repo, &ctx.gitai, &config, human, branch)?;
    gate::drop_sessions_without_record(ctx, &mut analysis)?;
    let (_, problems) = gate::check_sessions(ctx, &analysis)?;
    for p in problems {
        report.fail(None, p);
    }
    for item in &analysis.items {
        report.fail(None, msg::check::needs_approval(item.describe()));
    }
    report.items = analysis.items;
    Ok(())
}

/// `prov check <base>..<tip>`: commits that are already on a human branch.
/// No local hooks and no live state, only what a clone holds.
///
/// It checks the first-parent line of the range. A merge that is not an
/// integration brings in another human branch, for example a feature branch
/// with its own integrations, so the check follows that branch's first-parent
/// line in the same way.
fn check_range(ctx: &Ctx, base: &str, tip: &str, report: &mut Report) -> Result<()> {
    let repo = &ctx.repo;
    for rev in [base, tip] {
        if repo.rev_parse(rev)?.is_none() {
            bail!(msg::check::no_commit(rev));
        }
    }
    let tip_config = match repo.config_at(tip) {
        Ok(Some(config)) => config,
        Ok(None) => {
            report.fail(None, msg::check::rev_no_config(tip));
            return Ok(());
        }
        Err(e) => {
            report.fail(None, format!("{e:#}"));
            return Ok(());
        }
    };
    if let Err(e) = tip_config.check_version(prov_core::VERSION) {
        report.fail(None, e.to_string());
        return Ok(());
    }
    let emails = prov_adapters::agent_emails(&tip_config);
    let notes = Notes::read(repo, base, tip, &tip_config.known_gaps)?;
    if notes.is_none() {
        report.warn(None, msg::check::NO_NOTES_REF);
    }
    let mut seen = BTreeSet::new();
    // Each entry is the first-parent line of one human branch.
    let mut lines = vec![(base.to_string(), tip.to_string())];
    while let Some((base, tip)) = lines.pop() {
        let line = repo.commits(["--first-parent", "--reverse", &format!("{base}..{tip}")])?;
        for commit in &line {
            if !seen.insert(commit.sha.clone()) {
                continue;
            }
            if !gate::is_human_commit(commit, &emails) {
                report.fail(Some(commit), msg::check::NOT_HUMAN_COMMIT);
                continue;
            }
            if commit.parents.len() >= 2 {
                lines.extend(check_merge(ctx, commit, &emails, report)?);
            } else {
                check_ordinary(ctx, commit, notes.as_ref(), report)?;
            }
        }
    }
    report.commits_checked = Some(seen.len());
    Ok(())
}

/// A merge on a human branch. If it brings in agent commits, it must have
/// the trailers that the gate writes. A merge without them is how a forge
/// merge appears.
///
/// Returns the ranges `(base, tip)` of the human branches that the merge
/// brings in, for the caller to check.
fn check_merge(
    ctx: &Ctx,
    commit: &CommitInfo,
    emails: &[String],
    report: &mut Report,
) -> Result<Vec<(String, String)>> {
    let repo = &ctx.repo;
    let first = &commit.parents[0];
    let mut integrated = Vec::new();
    let mut human = Vec::new();
    for parent in &commit.parents[1..] {
        if gate::is_integration(repo, first, parent, emails)? {
            integrated.push(parent);
        } else {
            human.push((first.clone(), parent.clone()));
        }
    }
    let Some(incoming) = integrated.first() else { return Ok(human) };
    if commit.parents.len() > 2 {
        report.fail(Some(commit), msg::check::OCTOPUS);
        return Ok(human);
    }
    // The config for these checks is the file at the first parent.
    let config: Config = match repo.config_at(first) {
        Ok(Some(config)) => config,
        Ok(None) => {
            report.fail(Some(commit), msg::check::PARENT_NO_CONFIG);
            return Ok(human);
        }
        Err(e) => {
            report.fail(Some(commit), format!("{e:#}"));
            return Ok(human);
        }
    };
    let mut analysis = gate::analyze(repo, &ctx.gitai, &config, first, incoming)?;
    let groups = trailer::session_groups(&commit.trailers);
    // No live state here: a session that only a note names is known by its
    // trailer group. Without one, its lines need a Prov-Approved trailer.
    analysis.drop_unknown_sessions(|s| groups.iter().any(|(g, _)| g == s));
    if !trailer::has(&commit.trailers, trailer::PROV_VERSION) {
        report.fail(Some(commit), msg::check::NO_VERSION_TRAILER);
    }
    for session in &analysis.sessions {
        match groups.iter().find(|(s, _)| s == session) {
            None => report.fail(Some(commit), msg::check::no_trailer_group(session)),
            Some((_, hash)) if !hash.as_deref().is_some_and(trailer::is_sha256) => {
                report.fail(Some(commit), msg::check::no_transcript_hash(session));
            }
            Some(_) => {}
        }
    }
    let approvals = trailer::approvals(&commit.trailers);
    for item in gate::unapproved(&analysis.items, &approvals) {
        report.fail(Some(commit), msg::check::no_approval(item.describe()));
    }
    Ok(human)
}

/// Any other commit: its note names no AI session, except for a path with a
/// `Copied-From` trailer or a `Prov-Approved` trailer with the choice `ai`.
/// This is how a squash or a cherry-pick of agent commits appears, and the
/// work of a session in a human worktree.
///
/// A commit that adds lines and has no note gets a warning: a squash made on
/// a forge looks like this, and so does a commit by a person who does not use
/// git-ai.
fn check_ordinary(ctx: &Ctx, commit: &CommitInfo, notes: Option<&Notes>, report: &mut Report) -> Result<()> {
    let authorship = ctx.gitai.diff(&ctx.repo.toplevel, &commit.sha)?;
    if !authorship.added.is_empty() && notes.is_some_and(|n| n.missing(&commit.sha)) {
        report.warn(Some(commit), msg::check::NO_NOTE);
    }
    if authorship.sessions.is_empty() {
        return Ok(());
    }
    let mut exempt: Vec<String> = trailer::approvals(&commit.trailers)
        .into_iter()
        .filter(|a| a.choice == Choice::Ai)
        .map(|a| a.path)
        .collect();
    exempt.extend(
        commit
            .trailers
            .iter()
            .filter(|t| t.key == trailer::COPIED_FROM)
            .filter_map(|t| trailer::copied_from_dest(&t.value))
            .map(String::from),
    );
    let mut reported: Vec<(String, String)> = Vec::new();
    for range in &authorship.added {
        let Author::Ai(session) = &range.author else { continue };
        if exempt.contains(&range.path) {
            continue;
        }
        let key = (range.path.clone(), format!("{}:{}", session.tool, session.id));
        if !reported.contains(&key) {
            report.fail(
                Some(commit),
                msg::check::ai_lines_in_human_commit(&key.0, &key.1),
            );
            reported.push(key);
        }
    }
    Ok(())
}
