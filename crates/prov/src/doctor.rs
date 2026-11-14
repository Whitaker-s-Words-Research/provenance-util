//! `prov doctor`: checks of the local repository. It is not for CI; CI runs
//! `prov check <base>..<tip>`.

use std::path::{Path, PathBuf};

use anyhow::Result;
use prov_messages as msg;
use prov_core::config::CONFIG_FILE;
use prov_core::{now, Config, CopyCheck, State};
use prov_git::{shim, HOOK_NAMES};
use prov_gitai::{Compat, BASELINE_VERSION, TESTED_VERSION};
use serde::Serialize;

use crate::context::Ctx;
use crate::{init, session};

#[derive(Debug, Serialize)]
struct Check {
    name: &'static str,
    ok: bool,
    /// What is wrong, and how to correct it. Empty if `ok`.
    problems: Vec<String>,
    /// Information that is not a failure.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<String>,
    /// A risk that is not a failure: the check passes, and the person should
    /// act on it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Report {
    ok: bool,
    checks: Vec<Check>,
}

struct Checks(Vec<Check>);

impl Checks {
    fn add(&mut self, name: &'static str, problems: Vec<String>) {
        self.add_with_notes(name, problems, Vec::new());
    }

    fn add_with_notes(&mut self, name: &'static str, problems: Vec<String>, notes: Vec<String>) {
        self.add_full(name, problems, Vec::new(), notes);
    }

    fn add_full(&mut self, name: &'static str, problems: Vec<String>, warnings: Vec<String>, notes: Vec<String>) {
        self.0.push(Check { name, ok: problems.is_empty(), problems, notes, warnings });
    }
}

pub fn run(deep: bool, json: bool) -> Result<bool> {
    let ctx = Ctx::here()?;
    let repo = &ctx.repo;
    let mut checks = Checks(Vec::new());

    // 1. The config and the version.
    let mut problems = Vec::new();
    let config = if let Ok(text) = std::fs::read_to_string(repo.toplevel.join(CONFIG_FILE)) { match Config::parse(&text) {
        Ok(config) => {
            if let Err(e) = config.check_version(prov_core::VERSION) {
                problems.push(msg::doctor::version_problem(e));
            }
            Some(config)
        }
        Err(e) => {
            problems.push(format!("{e:#}"));
            None
        }
    } } else {
        problems.push(msg::doctor::no_config(CONFIG_FILE, repo.toplevel.display()));
        None
    };
    checks.add("config", problems);
    let config = config.unwrap_or_default();

    // 2. The hooks directory and the shims.
    let mut problems = Vec::new();
    if let Some(problem) = init::hooks_dir_problem(repo)? {
        problems.push(problem);
    }
    let hooks = repo.hooks_dir()?;
    for name in HOOK_NAMES {
        match std::fs::read_to_string(hooks.join(name)) {
            Ok(text) if text == shim(name) => {
                if !is_executable(&hooks.join(name)) {
                    problems.push(msg::doctor::shim_not_executable(name));
                }
            }
            Ok(_) => problems.push(msg::doctor::not_shim(hooks.join(name).display())),
            Err(_) => problems.push(msg::doctor::no_shim(name, hooks.display())),
        }
    }
    checks.add("hooks", problems);

    // 3. The git options.
    let mut problems = Vec::new();
    for (key, value) in init::GIT_OPTIONS {
        let found = repo.git_opt(["config", "--local", "--get", key])?;
        if found.as_deref() != Some(value) {
            problems.push(msg::doctor::git_option(key, found, value));
        }
    }
    checks.add("git options", problems);

    // 4. The agent environment, and 5. the tracked files.
    let mut environment = Vec::new();
    let mut tracked = Vec::new();
    let mut files = vec![CONFIG_FILE.to_string()];
    for agent in &config.agents {
        let Some(adapter) = prov_adapters::by_agent_name(agent) else {
            environment.push(msg::doctor::no_adapter(agent));
            continue;
        };
        let Some(identity) = prov_adapters::identity(&config, agent) else {
            environment.push(msg::doctor::no_identity(agent));
            continue;
        };
        let file = adapter.settings_file();
        let text = std::fs::read_to_string(repo.toplevel.join(file)).ok();
        for problem in adapter.settings_problems(text.as_deref(), &identity) {
            environment.push(msg::doctor::settings_problem(file, problem));
        }
        files.push(file.to_string());
    }
    for file in &files {
        if !repo.toplevel.join(file).is_file() {
            continue; // reported above
        }
        if !repo.git_ok(["ls-files", "--error-unmatch", "--", file])? {
            tracked.push(msg::doctor::not_tracked(file));
        } else if !repo.git_ok(["diff", "--quiet", "HEAD", "--", file])? {
            tracked.push(msg::doctor::not_committed(file));
        }
    }
    checks.add("agent environment", environment);
    checks.add("tracked files", tracked);

    // 6. git-ai.
    let mut problems = Vec::new();
    let mut warnings = Vec::new();
    let mut notes = Vec::new();
    match ctx.gitai.version(&repo.toplevel) {
        Err(e) => problems.push(msg::doctor::no_gitai(format!("{e:#}"))),
        Ok(version) => {
            match prov_gitai::compat(&version) {
                Compat::Unsupported => problems.push(msg::doctor::gitai_unsupported(&version, BASELINE_VERSION)),
                Compat::Untested => warnings.push(msg::doctor::gitai_untested(&version, TESTED_VERSION)),
                Compat::Tested => {}
            }
            if !ctx.gitai.service_available(&repo.toplevel) {
                problems.push(msg::doctor::GITAI_UNAVAILABLE.to_string());
            }
            match allowed(&ctx) {
                Ok(None) => {}
                Ok(Some(problem)) => problems.push(problem),
                Err(e) => problems.push(msg::doctor::cannot_read_gitai_config(format!("{e:#}"))),
            }
            if deep {
                if let Err(output) = ctx.gitai.debug(&repo.toplevel) {
                    problems.push(msg::doctor::gitai_debug_fails(output));
                }
            } else {
                notes.push(msg::doctor::DEBUG_ONLY_DEEP.to_string());
            }
        }
    }
    checks.add_full("git-ai", problems, warnings, notes);

    // 7. The git-ai integration of each agent.
    let mut problems = Vec::new();
    match std::env::var_os("HOME").map(PathBuf::from) {
        None => problems.push(msg::completions::HOME_NOT_SET.to_string()),
        Some(home) => {
            for agent in &config.agents {
                if let Some(adapter) = prov_adapters::by_agent_name(agent) {
                    if let Err(problem) = adapter.gitai_integration(&home) {
                        problems.push(msg::doctor::integration_missing(agent, problem));
                    }
                }
            }
        }
    }
    checks.add("git-ai integration", problems);

    // 8. Sessions, worktrees and copies.
    let mut problems = Vec::new();
    let views = session::views(&ctx)?;
    let mut merged = Vec::new();
    for v in &views {
        let r = &v.record;
        // An integration deletes only the records that its trailers name. A
        // record of another session whose branch is merged stays (R-INT-2).
        if r.state != State::Open && !ctx.on_agent_branch() {
            if let Some(tip) = ctx.repo.rev_parse(&format!("refs/heads/{}", r.branch))? {
                if ctx.repo.is_ancestor(&tip, "HEAD")? {
                    merged.push(msg::doctor::session_merged(&r.id, r.state.as_str(), &r.branch));
                }
            }
        }
        if v.stale {
            problems.push(msg::doctor::session_stale(&r.id));
        }
        if r.state == State::Failed {
            problems.push(msg::doctor::session_failed(&r.id, r.error.as_deref().unwrap_or(msg::gate::NO_ERROR_TEXT)));
        }
    }
    let mut contended: Vec<&Path> = views.iter().filter(|v| v.contended).map(|v| v.record.worktree.as_path()).collect();
    contended.sort();
    contended.dedup();
    for worktree in contended {
        let ids: Vec<&str> =
            views.iter().filter(|v| v.contended && v.record.worktree == worktree).map(|v| v.record.id.as_str()).collect();
        problems.push(msg::doctor::worktree_contended(worktree.display(), ids.join(", ")));
    }
    // A copy stays in `copies/` until its check agrees for every file.
    let mut notes = merged;
    // The owner of a worktree that git no longer lists is of no use. Such a
    // record stays when a worktree is removed without `prov finish`. This is
    // the one thing that `prov doctor` changes.
    let listed: Vec<PathBuf> = ctx.repo.worktrees()?.into_iter().map(|w| w.path).collect();
    let gone: Vec<_> = ctx.store.worktree_records()?.into_iter().filter(|w| !listed.contains(&w.worktree)).collect();
    if !gone.is_empty() {
        let lock = ctx.store.lock()?;
        for w in &gone {
            lock.delete_worktree(&w.worktree)?;
            notes.push(msg::doctor::worktree_record_removed(w.worktree.display()));
        }
    }
    for copy in ctx.store.copies()? {
        let staged = copy.files.iter().filter(|f| f.commit.is_none()).count();
        if staged > 0 {
            notes.push(msg::doctor::copy_not_committed(staged, &copy.id, copy.worktree.display()));
        }
        for file in &copy.files {
            let detail = file.detail.as_deref().unwrap_or(msg::doctor::NO_DETAIL);
            let commit = file.commit.as_deref().unwrap_or("");
            match file.check {
                Some(CopyCheck::Failed) => problems.push(msg::doctor::copy_failed(&file.dest, commit, detail)),
                Some(CopyCheck::Moved) => {
                    notes.push(msg::doctor::copy_moved(&file.dest, commit, detail, ctx.store.copy_path(&copy.id)?.display()));
                }
                Some(CopyCheck::Ignored) => {
                    notes.push(msg::doctor::copy_ignored(&file.dest, commit, detail, ctx.store.copy_path(&copy.id)?.display()));
                }
                // The check waits for the note for a maximum of 60 seconds.
                None if file.committed_at.is_some_and(|at| now().saturating_sub(at) > 120) => problems.push(msg::doctor::copy_unfinished(&file.dest, commit, &copy.id)),
                // Agrees, or the check still runs.
                Some(CopyCheck::Agrees) | None => {}
            }
        }
    }
    checks.add_with_notes("sessions", problems, notes);

    // 9. Commits with no git-ai note, on human branches and agent branches.
    let (problems, notes) = note_gaps(&ctx, &config)?;
    checks.add_with_notes("git-ai notes", problems, notes);

    let report = Report { ok: checks.0.iter().all(|c| c.ok), checks: checks.0 };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for check in &report.checks {
            println!("{}", if check.ok { msg::doctor::check_ok(check.name) } else { msg::doctor::check_fail(check.name) });
            for problem in &check.problems {
                for (i, line) in problem.lines().enumerate() {
                    println!("{}{line}", if i == 0 { msg::doctor::PROBLEM_FIRST } else { msg::doctor::PROBLEM_NEXT });
                }
            }
            for warning in &check.warnings {
                for (i, line) in warning.lines().enumerate() {
                    println!("{}{line}", if i == 0 { msg::doctor::WARNING_FIRST } else { msg::doctor::WARNING_NEXT });
                }
            }
            for note in &check.notes {
                println!("{}", msg::doctor::note(note));
            }
        }
        println!("{}", if report.ok { msg::doctor::ALL_PASS } else { msg::doctor::SOME_FAIL });
    }
    Ok(report.ok)
}

/// The commits of the local branches that add lines and have no git-ai note.
///
/// git-ai writes a note for every commit that its service sees, so a commit
/// with no note is one that git-ai did not see: the service was not running,
/// or the commit was made where git-ai is not installed. If a commit that no
/// branch contains has the same patch and a note, the missing note is the
/// result of a rewrite (an amend, a rebase) that git-ai did not see.
///
/// Commits older than the first commit of `refs/notes/ai` are not checked:
/// they were made before git-ai was in use. Merge commits are not checked.
fn note_gaps(ctx: &Ctx, config: &Config) -> Result<(Vec<String>, Vec<String>)> {
    use std::collections::{BTreeMap, BTreeSet};

    let repo = &ctx.repo;
    let mut problems = Vec::new();
    let mut notes = Vec::new();
    if repo.rev_parse("refs/notes/ai")?.is_none() {
        notes.push(msg::doctor::NO_NOTES_REF.to_string());
        return Ok((problems, notes));
    }
    // A note of a recent commit can still be on its way.
    if ctx.gitai.service_available(&repo.toplevel) {
        if let Err(e) = ctx.gitai.await_notes(&repo.toplevel) {
            notes.push(msg::doctor::await_failed(format!("{e:#}")));
        }
    }
    let since: u64 = repo
        .git(["log", "--max-parents=0", "--format=%ct", "refs/notes/ai"])?
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .min()
        .unwrap_or(0);
    let with_note: BTreeSet<String> = repo
        .git(["notes", "--ref=ai", "list"])?
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1).map(str::to_string))
        .collect();
    let branches: Vec<String> = repo
        .git(["for-each-ref", "--format=%(refname:short)", "refs/heads"])?
        .lines()
        .map(str::to_string)
        .collect();
    if branches.is_empty() {
        return Ok((problems, notes));
    }
    let human: Vec<&String> = branches.iter().filter(|b| !prov_core::pattern::is_agent_branch(b)).collect();
    let on_human: BTreeSet<String> = if human.is_empty() {
        BTreeSet::new()
    } else {
        let mut args = vec!["rev-list".to_string(), "--no-merges".to_string()];
        args.extend(human.iter().map(|b| format!("refs/heads/{b}")));
        repo.git(&args)?.lines().map(str::to_string).collect()
    };

    let mut gaps = Vec::new();
    for line in repo.git(["log", "--branches", "--no-merges", "--format=%H %ct"])?.lines() {
        let Some((commit, time)) = line.split_once(' ') else { continue };
        if time.trim().parse::<u64>().unwrap_or(0) < since || with_note.contains(commit) {
            continue;
        }
        if !adds_lines(repo, commit)? {
            continue;
        }
        gaps.push(commit.to_string());
    }
    if gaps.is_empty() && config.known_gaps.is_empty() {
        return Ok((problems, notes));
    }

    // Commits with a note that no branch contains: the old commits of a
    // rewrite. They are found by their patch.
    let reachable: BTreeSet<String> = repo.git(["rev-list", "--branches"])?.lines().map(str::to_string).collect();
    let mut by_patch: BTreeMap<String, String> = BTreeMap::new();
    if !gaps.is_empty() {
        for old in with_note.iter().filter(|c| !reachable.contains(*c)) {
            if !repo.git_ok(["cat-file", "-e", &format!("{old}^{{commit}}")])? {
                continue; // the object is gone
            }
            if let Some(id) = patch_id(repo, old)? {
                by_patch.entry(id).or_insert_with(|| old.clone());
            }
        }
    }

    let accepted = |commit: &str| config.known_gaps.iter().any(|g| commit.starts_with(&g.to_ascii_lowercase()));
    for commit in &gaps {
        let kind = if on_human.contains(commit) { msg::doctor::HUMAN_BRANCH } else { msg::doctor::AGENT_BRANCH };
        let subject = repo.git(["log", "-1", "--format=%s", commit])?;
        if accepted(commit) {
            notes.push(msg::doctor::gap_accepted(commit, &subject));
            continue;
        }
        let accept = msg::doctor::gap_accept(commit, CONFIG_FILE);
        let replace = msg::doctor::GAP_REPLACE;
        match patch_id(repo, commit)?.and_then(|id| by_patch.get(&id)) {
            Some(old) => problems.push(msg::doctor::gap_rewrite(commit, kind, &subject, old, replace, &accept)),
            None => problems.push(msg::doctor::gap(commit, kind, &subject, replace, &accept)),
        }
    }
    for gap in &config.known_gaps {
        let gap = gap.to_ascii_lowercase();
        if !gaps.iter().any(|c| c.starts_with(&gap)) {
            notes.push(msg::doctor::stale_known_gap(&gap));
        }
    }
    Ok((problems, notes))
}

/// True if the commit adds at least one line, or changes a binary file.
fn adds_lines(repo: &prov_git::Repo, commit: &str) -> Result<bool> {
    let numstat = repo.git(["diff-tree", "--root", "--no-commit-id", "-r", "--numstat", commit])?;
    Ok(numstat.lines().any(|l| l.split('\t').next().is_some_and(|added| added != "0")))
}

/// `git patch-id --stable` of a commit, or `None` for a commit with no diff.
fn patch_id(repo: &prov_git::Repo, commit: &str) -> Result<Option<String>> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let diff = repo.git_bytes(["diff-tree", "--root", "-p", commit])?;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(&repo.toplevel)
        .args(["patch-id", "--stable"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    child.stdin.take().ok_or_else(|| std::io::Error::other(msg::gitai::NO_STDIN))?.write_all(&diff)?;
    let out = child.wait_with_output()?;
    Ok(String::from_utf8_lossy(&out.stdout).split_whitespace().next().map(str::to_string))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

/// Why git-ai does not allow this repository, if it does not.
///
/// git-ai matches `allow_repositories` and `exclude_repositories` against
/// remote URLs only. A repository with no remote is not allowed when the
/// allow list has entries. In a repository that is not allowed, git-ai
/// ignores checkpoints and prints only a message.
pub(crate) fn allowed(ctx: &Ctx) -> Result<Option<String>> {
    let dir = &ctx.repo.toplevel;
    let allow = ctx.gitai.config_list(dir, "allow_repositories")?;
    let exclude = ctx.gitai.config_list(dir, "exclude_repositories")?;
    let mut urls = Vec::new();
    for remote in ctx.repo.git(["remote"])?.lines() {
        if let Some(url) = ctx.repo.git_opt(["remote", "get-url", remote])? {
            urls.push(url);
        }
    }
    let matches = |patterns: &[String], url: &str| patterns.iter().any(|p| remote_matches(p, url));
    if let Some(url) = urls.iter().find(|u| matches(&exclude, u)) {
        return Ok(Some(msg::doctor::excluded(url)));
    }
    if !allow.is_empty() && !urls.iter().any(|u| matches(&allow, u)) {
        return Ok(Some(msg::doctor::not_allowed(allow.join(", "), urls.is_empty())));
    }
    Ok(None)
}

/// `host/owner/repo` from a remote URL: no scheme, no user, no `.git`.
fn normalize_remote(url: &str) -> String {
    let mut text = url.trim();
    if let Some((_, rest)) = text.split_once("://") {
        text = rest;
    }
    if let Some((user, rest)) = text.split_once('@') {
        if !user.contains('/') {
            text = rest;
        }
    }
    let text = text.strip_suffix(".git").unwrap_or(text).trim_end_matches('/');
    // The scp form `host:owner/repo`.
    match text.split_once(':') {
        Some((host, path)) if !host.contains('/') && !path.starts_with(|c: char| c.is_ascii_digit()) => {
            format!("{host}/{path}")
        }
        _ => text.to_string(),
    }
}

/// True if a pattern of git-ai's repository lists matches a remote URL.
/// `*` matches any characters. The comparison ignores the scheme, the user
/// and `.git`, in the pattern too.
fn remote_matches(pattern: &str, url: &str) -> bool {
    fn glob(pattern: &[u8], text: &[u8]) -> bool {
        match pattern.split_first() {
            None => text.is_empty(),
            Some((b'*', rest)) => (0..=text.len()).any(|skip| glob(rest, &text[skip..])),
            Some((c, rest)) => text.first().is_some_and(|t| t.eq_ignore_ascii_case(c)) && glob(rest, &text[1..]),
        }
    }
    pattern == url || glob(normalize_remote(pattern).as_bytes(), normalize_remote(url).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_patterns() {
        let pattern = "github.com/Example-Org/*";
        assert!(remote_matches(pattern, "git@github.com:Example-Org/tool.git"));
        assert!(remote_matches(pattern, "https://github.com/example-org/tool"));
        assert!(remote_matches(pattern, "ssh://git@github.com/Example-Org/tool.git"));
        assert!(!remote_matches(pattern, "git@github.com:Other/tool.git"));
        assert!(!remote_matches(pattern, "/home/me/tool"));
        assert!(remote_matches("*", "/home/me/tool"));
        assert_eq!(normalize_remote("https://example.invalid:8443/a/b.git"), "example.invalid:8443/a/b");
    }
}
