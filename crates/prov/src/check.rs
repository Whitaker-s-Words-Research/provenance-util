//! `prov check [<agent branch> | <base>..<tip>]`: the rules of the merge
//! gate without a merge. It changes nothing.

use anyhow::{bail, Context, Result};
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
    /// Form with a range: the number of first-parent commits checked.
    #[serde(skip_serializing_if = "Option::is_none")]
    commits_checked: Option<usize>,
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
}

/// Returns false if one check fails.
pub fn run(target: Option<&str>, json: bool) -> Result<bool> {
    let ctx = Ctx::here()?;
    let mut report = Report::default();
    match target {
        None => list_branches(&ctx, &mut report)?,
        Some(t) if t.contains("...") => bail!("give a range as <base>..<tip>"),
        Some(t) => match t.split_once("..") {
            Some((base, tip)) => check_range(&ctx, base, tip, &mut report)?,
            None => check_branch(&ctx, t, &mut report)?,
        },
    }
    report.ok = report.failures.is_empty();
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print(&report, target);
    }
    Ok(report.ok)
}

fn print(report: &Report, target: Option<&str>) {
    for b in &report.branches {
        println!("{}", b.branch);
        println!("  commits: {}", b.commits.len());
        for c in &b.commits {
            println!("    {c}");
        }
        let sessions: Vec<String> = b.sessions.iter().map(|s| format!("{} ({})", s.session, s.state)).collect();
        println!("  sessions: {}", if sessions.is_empty() { "none".into() } else { sessions.join(", ") });
        println!("  doubtful lines: {}", b.doubtful_lines);
        if !b.protected_paths.is_empty() {
            println!("  protected paths changed: {}", b.protected_paths.join(", "));
        }
    }
    if target.is_none() && report.branches.is_empty() && report.failures.is_empty() {
        println!("No agent branch waits for integration.");
    }
    for f in &report.failures {
        match &f.commit {
            Some(c) => println!("FAIL {} {}", &c[..c.len().min(10)], f.message),
            None => println!("FAIL {}", f.message),
        }
    }
    if let Some(n) = report.commits_checked {
        println!("{n} commit(s) checked, {} failure(s).", report.failures.len());
    } else if target.is_some() && report.ok {
        println!("OK: the merge gate has nothing to refuse.");
    }
}

fn session_state(ctx: &Ctx, session: &trailer::SessionRef) -> String {
    match ctx.store.get(&session.id).ok().flatten().filter(|r| r.agent == session.agent) {
        Some(r) => r.state.as_str().to_string(),
        None => "no record".to_string(),
    }
}

/// `prov check`: what waits.
fn list_branches(ctx: &Ctx, report: &mut Report) -> Result<()> {
    let repo = &ctx.repo;
    let here = repo.config_here()?;
    let human = here.human_branch.clone();
    if repo.rev_parse(&human)?.is_none() {
        report.fail(None, format!("the human branch `{human}` does not exist"));
        return Ok(());
    }
    let config = repo.config_at(&human)?.unwrap_or(here);
    // git-ai writes a note some seconds after a commit. Without the note,
    // every line of a new commit would count as doubtful.
    if let Err(e) = ctx.gitai.await_notes(&repo.toplevel) {
        report.fail(None, format!("{e:#}"));
        return Ok(());
    }
    let refs = repo.git(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])?;
    for branch in refs.lines().filter(|b| is_agent_branch(b)) {
        if repo.is_ancestor(branch, &human)? {
            continue;
        }
        let analysis = gate::analyze(repo, &ctx.gitai, &config, &human, branch)?;
        report.branches.push(BranchReport {
            branch: branch.to_string(),
            commits: analysis.commits.iter().map(|c| format!("{} {}", c.short(), c.subject)).collect(),
            sessions: analysis
                .sessions
                .iter()
                .map(|s| SessionStatus { session: s.to_string(), state: session_state(ctx, s) })
                .collect(),
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

/// `prov check <agent branch>`: steps 2 to 4 of the gate for that branch.
fn check_branch(ctx: &Ctx, branch: &str, report: &mut Report) -> Result<()> {
    let repo = &ctx.repo;
    let human = repo.config_here()?.human_branch;
    if repo.rev_parse(branch)?.is_none() {
        bail!("no branch or commit `{branch}`");
    }
    let Some(config) = repo.config_at(&human).with_context(|| format!("the config of `{human}`"))? else {
        report.fail(None, format!("the human branch `{human}` has no .provenance.toml"));
        return Ok(());
    };
    if let Err(e) = config.check_version(prov_core::VERSION) {
        report.fail(None, e.to_string());
    }
    if let Err(e) = ctx.gitai.await_notes(&repo.toplevel) {
        report.fail(None, format!("{e:#}"));
        return Ok(());
    }
    let analysis = gate::analyze(repo, &ctx.gitai, &config, &human, branch)?;
    let (_, problems) = gate::check_sessions(ctx, &analysis)?;
    for p in problems {
        report.fail(None, p);
    }
    for item in &analysis.items {
        report.fail(None, format!("needs approval at the merge: {}", item.describe()));
    }
    report.items = analysis.items;
    Ok(())
}

/// `prov check <base>..<tip>`: commits that are already on the human branch.
/// No local hooks and no live state, only what a clone holds.
fn check_range(ctx: &Ctx, base: &str, tip: &str, report: &mut Report) -> Result<()> {
    let repo = &ctx.repo;
    for rev in [base, tip] {
        if repo.rev_parse(rev)?.is_none() {
            bail!("no commit `{rev}`");
        }
    }
    let tip_config = match repo.config_at(tip) {
        Ok(Some(config)) => config,
        Ok(None) => {
            report.fail(None, format!("{tip} has no .provenance.toml"));
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
    let line = repo.commits(["--first-parent", "--reverse", &format!("{base}..{tip}")])?;
    report.commits_checked = Some(line.len());

    for commit in &line {
        if !gate::is_human_commit(commit, &emails) {
            report.fail(
                Some(commit),
                "is not a human commit, on the first-parent line of the human branch (a fast-forward, rebase or \
                 cherry-pick of agent commits?)",
            );
            continue;
        }
        if commit.parents.len() >= 2 {
            check_merge(ctx, commit, &emails, report)?;
        } else {
            check_ordinary(ctx, commit, report)?;
        }
    }
    Ok(())
}

/// A merge on the human branch. If it brings in agent commits, it must have
/// the trailers that the gate writes. A merge without them is how a forge
/// merge appears.
fn check_merge(ctx: &Ctx, commit: &CommitInfo, emails: &[String], report: &mut Report) -> Result<()> {
    let repo = &ctx.repo;
    let first = &commit.parents[0];
    let mut integrated = Vec::new();
    for parent in &commit.parents[1..] {
        if gate::is_integration(repo, first, parent, emails)? {
            integrated.push(parent);
        }
    }
    let Some(incoming) = integrated.first() else { return Ok(()) };
    if commit.parents.len() > 2 {
        report.fail(Some(commit), "is an octopus merge that contains agent commits");
        return Ok(());
    }
    // The config for these checks is the file at the first parent.
    let config: Config = match repo.config_at(first) {
        Ok(Some(config)) => config,
        Ok(None) => {
            report.fail(Some(commit), "brings in agent commits, but its first parent has no .provenance.toml");
            return Ok(());
        }
        Err(e) => {
            report.fail(Some(commit), format!("{e:#}"));
            return Ok(());
        }
    };
    let analysis = gate::analyze(repo, &ctx.gitai, &config, first, incoming)?;
    if !trailer::has(&commit.trailers, trailer::PROV_VERSION) {
        report.fail(
            Some(commit),
            "brings in agent commits and has no Prov-Version trailer: it did not pass the merge gate (a merge on a forge?)",
        );
    }
    let groups = trailer::session_groups(&commit.trailers);
    for session in &analysis.sessions {
        match groups.iter().find(|(s, _)| s == session) {
            None => report.fail(Some(commit), format!("has no trailer group for the session {session}")),
            Some((_, hash)) if !hash.as_deref().is_some_and(trailer::is_sha256) => {
                report.fail(Some(commit), format!("has no valid Prov-Transcript-SHA256 for the session {session}"))
            }
            Some(_) => {}
        }
    }
    let approvals = trailer::approvals(&commit.trailers);
    for item in gate::unapproved(&analysis.items, &approvals) {
        report.fail(Some(commit), format!("has no Prov-Approved trailer for {}", item.describe()));
    }
    Ok(())
}

/// Any other commit: its note names no AI session, except for a path with a
/// `Copied-From` trailer or a `Prov-Approved` trailer with the choice `ai`.
/// This is how a squash or a cherry-pick of agent commits appears, and the
/// work of a session in a human worktree.
fn check_ordinary(ctx: &Ctx, commit: &CommitInfo, report: &mut Report) -> Result<()> {
    let authorship = ctx.gitai.diff(&ctx.repo.toplevel, &commit.sha)?;
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
            .filter_map(|t| trailer::copied_from_path(&t.value))
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
                format!(
                    "is a human commit, but its note gives lines of {} to the AI session {} (a squash or cherry-pick \
                     of agent commits, or a session in a human worktree?)",
                    key.0, key.1
                ),
            );
            reported.push(key);
        }
    }
    Ok(())
}
