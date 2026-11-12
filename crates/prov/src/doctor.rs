//! `prov doctor`: checks of the local repository. It is not for CI; CI runs
//! `prov check <base>..<tip>`.

use std::path::{Path, PathBuf};

use anyhow::Result;
use prov_core::config::CONFIG_FILE;
use prov_core::{Config, State};
use prov_git::{shim, HOOK_NAMES};
use serde::Serialize;

use crate::context::Ctx;
use crate::{init, session};

/// The compatibility baseline of git-ai.
const GITAI_BASELINE: (u64, u64, u64) = (1, 7, 6);

#[derive(Debug, Serialize)]
struct Check {
    name: &'static str,
    ok: bool,
    /// What is wrong, and how to correct it. Empty if `ok`.
    problems: Vec<String>,
    /// Information that is not a failure.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<String>,
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
        self.0.push(Check { name, ok: problems.is_empty(), problems, notes });
    }
}

pub fn run(deep: bool, json: bool) -> Result<bool> {
    let ctx = Ctx::here()?;
    let repo = &ctx.repo;
    let mut checks = Checks(Vec::new());

    // 1. The config and the version.
    let mut problems = Vec::new();
    let config = match std::fs::read_to_string(repo.toplevel.join(CONFIG_FILE)) {
        Ok(text) => match Config::parse(&text) {
            Ok(config) => {
                if let Err(e) = config.check_version(prov_core::VERSION) {
                    problems.push(format!("{e}. Install that version, or change required_version."));
                }
                Some(config)
            }
            Err(e) => {
                problems.push(format!("{e:#}"));
                None
            }
        },
        Err(_) => {
            problems.push(format!("{CONFIG_FILE} does not exist in {}. Run: prov init", repo.toplevel.display()));
            None
        }
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
                    problems.push(format!("the shim {name} is not executable. Run: prov init"));
                }
            }
            Ok(_) => problems.push(format!("{} is not the shim of prov. Run: prov init", hooks.join(name).display())),
            Err(_) => problems.push(format!("the shim {name} does not exist in {}. Run: prov init", hooks.display())),
        }
    }
    checks.add("hooks", problems);

    // 3. The git options.
    let mut problems = Vec::new();
    for (key, value) in init::GIT_OPTIONS {
        let found = repo.git_opt(["config", "--local", "--get", key])?;
        if found.as_deref() != Some(value) {
            problems.push(format!(
                "{key} is {} in this repository, not `{value}`. Run: prov init",
                found.map_or("not set".to_string(), |f| format!("`{f}`"))
            ));
        }
    }
    checks.add("git options", problems);

    // 4. The agent environment, and 5. the tracked files.
    let mut environment = Vec::new();
    let mut tracked = Vec::new();
    let mut files = vec![CONFIG_FILE.to_string()];
    for agent in &config.agents {
        let Some(adapter) = prov_adapters::by_agent_name(agent) else {
            environment.push(format!("no adapter for the agent `{agent}`"));
            continue;
        };
        let Some(identity) = prov_adapters::identity(&config, agent) else {
            environment.push(format!("no commit identity for the agent `{agent}`"));
            continue;
        };
        let file = adapter.settings_file();
        let text = std::fs::read_to_string(repo.toplevel.join(file)).ok();
        for problem in adapter.settings_problems(text.as_deref(), &identity) {
            environment.push(format!("{file}: {problem}. Run: prov init"));
        }
        files.push(file.to_string());
    }
    for file in &files {
        if !repo.toplevel.join(file).is_file() {
            continue; // reported above
        }
        if !repo.git_ok(["ls-files", "--error-unmatch", "--", file])? {
            tracked.push(format!("{file} is not tracked. Commit it: an agent worktree gets it only from a commit."));
        } else if !repo.git_ok(["diff", "--quiet", "HEAD", "--", file])? {
            tracked.push(format!("{file} has changes that are not committed"));
        }
    }
    checks.add("agent environment", environment);
    checks.add("tracked files", tracked);

    // 6. git-ai.
    let mut problems = Vec::new();
    let mut notes = Vec::new();
    match ctx.gitai.version(&repo.toplevel) {
        Err(e) => problems.push(format!("{e:#}. Install git-ai.")),
        Ok(version) => {
            if !parse_version(&version).is_some_and(|v| v >= GITAI_BASELINE) {
                problems.push(format!(
                    "git-ai is version {version}. The compatibility baseline is {}.{}.{}.",
                    GITAI_BASELINE.0, GITAI_BASELINE.1, GITAI_BASELINE.2
                ));
            }
            if !ctx.gitai.service_available(&repo.toplevel) {
                problems.push("the git-ai service is not available. Start it: git-ai bg start".to_string());
            }
            match allowed(&ctx) {
                Ok(None) => {}
                Ok(Some(problem)) => problems.push(problem),
                Err(e) => problems.push(format!("cannot read the git-ai configuration: {e:#}")),
            }
            if deep {
                if let Err(output) = ctx.gitai.debug(&repo.toplevel) {
                    problems.push(format!("`git-ai debug` fails:\n{output}"));
                }
            } else {
                notes.push("`git-ai debug` runs only with --deep".to_string());
            }
        }
    }
    checks.add_with_notes("git-ai", problems, notes);

    // 7. The git-ai integration of each agent.
    let mut problems = Vec::new();
    match std::env::var_os("HOME").map(PathBuf::from) {
        None => problems.push("HOME is not set".to_string()),
        Some(home) => {
            for agent in &config.agents {
                if let Some(adapter) = prov_adapters::by_agent_name(agent) {
                    if let Err(problem) = adapter.gitai_integration(&home) {
                        problems.push(format!("the git-ai integration of `{agent}` is not installed: {problem}"));
                    }
                }
            }
        }
    }
    checks.add("git-ai integration", problems);

    // 8. Sessions, worktrees and copies.
    let mut problems = Vec::new();
    let views = session::views(&ctx)?;
    for v in &views {
        let r = &v.record;
        if v.stale {
            problems.push(format!(
                "session {} is stale (no event for 24 hours, or its agent process is gone). Close it: prov session close {}",
                r.id, r.id
            ));
        }
        if r.state == State::Failed {
            problems.push(format!(
                "session {} is failed: {}. Run the archive step again: prov session close {}",
                r.id,
                r.error.as_deref().unwrap_or("no error text"),
                r.id
            ));
        }
    }
    let mut contended: Vec<&Path> = views.iter().filter(|v| v.contended).map(|v| v.record.worktree.as_path()).collect();
    contended.sort();
    contended.dedup();
    for worktree in contended {
        let ids: Vec<&str> =
            views.iter().filter(|v| v.contended && v.record.worktree == worktree).map(|v| v.record.id.as_str()).collect();
        problems.push(format!(
            "the worktree {} is contended: the sessions {} are open there. End all but one.",
            worktree.display(),
            ids.join(", ")
        ));
    }
    // A plan stays in `copies/` only while its check is pending or failed.
    if let Ok(entries) = std::fs::read_dir(ctx.store.dir().join("copies")) {
        for entry in entries.flatten() {
            let path = entry.path();
            let failed = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .is_some_and(|v| v.get("failed").and_then(|f| f.as_bool()).unwrap_or(false));
            if failed {
                problems.push(format!("a copy check failed: {}", path.display()));
            }
        }
    }
    checks.add("sessions", problems);

    let report = Report { ok: checks.0.iter().all(|c| c.ok), checks: checks.0 };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for check in &report.checks {
            println!("{} {}", if check.ok { "ok  " } else { "FAIL" }, check.name);
            for problem in &check.problems {
                for (i, line) in problem.lines().enumerate() {
                    println!("{}{line}", if i == 0 { "       - " } else { "         " });
                }
            }
            for note in &check.notes {
                println!("       ({note})");
            }
        }
        println!("{}", if report.ok { "All checks pass." } else { "One check or more fails." });
    }
    Ok(report.ok)
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim_start_matches('v').split(['.', '-', '+']);
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// Why git-ai does not allow this repository, if it does not.
///
/// git-ai matches `allow_repositories` and `exclude_repositories` against
/// remote URLs only. A repository with no remote is not allowed when the
/// allow list has entries. In a repository that is not allowed, git-ai
/// ignores checkpoints and prints only a message.
fn allowed(ctx: &Ctx) -> Result<Option<String>> {
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
        return Ok(Some(format!("the remote {url} is in git-ai's exclude_repositories. git-ai ignores checkpoints here.")));
    }
    if !allow.is_empty() && !urls.iter().any(|u| matches(&allow, u)) {
        return Ok(Some(format!(
            "git-ai's allow_repositories ({}) matches no remote of this repository{}. git-ai ignores checkpoints here. \
             Add a remote that matches, or: git-ai config --add allow_repositories <pattern>",
            allow.join(", "),
            if urls.is_empty() { " (it has no remote)" } else { "" }
        )));
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

    #[test]
    fn versions() {
        assert_eq!(parse_version("1.7.6"), Some((1, 7, 6)));
        assert_eq!(parse_version("v1.10.0-beta"), Some((1, 10, 0)));
        assert!(parse_version("1.7") < Some(GITAI_BASELINE));
        assert!(parse_version("1.10.0") > Some(GITAI_BASELINE));
    }
}
