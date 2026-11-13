//! `prov init`: the config file, the hook shims, the agent settings and the
//! git options. A human runs it once in each clone. A second run changes
//! nothing that is already correct.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use prov_core::config::CONFIG_FILE;
use prov_core::Config;
use prov_git::{shim, Repo, HOOK_NAMES};
use serde::Serialize;

use crate::context::Ctx;

/// The git options that force a merge commit for every merge, and keep
/// `git pull` from making one.
pub const GIT_OPTIONS: [(&str, &str); 2] = [("merge.ff", "false"), ("pull.ff", "only")];

pub struct Args {
    pub agents: Vec<String>,
    pub force: bool,
    pub dry_run: bool,
    pub json: bool,
}

#[derive(Debug, Serialize)]
struct Report {
    dry_run: bool,
    /// What `prov init` did, or with `--dry-run` what it will do.
    actions: Vec<Action>,
    /// Tracked files that changed. The human commits them.
    to_commit: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Action {
    target: String,
    /// `written`, `kept`, `set` or `moved`.
    result: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

/// Why the hooks directory is not acceptable, if it is not.
///
/// The directory must be outside every worktree: during a merge, git runs
/// hooks after it writes the merged files, so a shim in a checkout would be
/// the agent branch's copy.
pub fn hooks_dir_problem(repo: &Repo) -> Result<Option<String>> {
    if let Some(value) = repo.config_value("core.hooksPath")? {
        let absolute = Path::new(&value).is_absolute() || value.starts_with("~/");
        if !absolute {
            return Ok(Some(format!(
                "core.hooksPath is the relative path `{value}`, which is inside each worktree. Use an absolute path \
                 outside every worktree, or unset it: git config --unset core.hooksPath"
            )));
        }
    }
    let dir = repo.hooks_dir()?;
    // `.git/hooks` is under the main worktree, but it is not part of a checkout.
    if dir.starts_with(&repo.common_dir) {
        return Ok(None);
    }
    for worktree in repo.worktrees()? {
        if dir.starts_with(&worktree.path) {
            return Ok(Some(format!(
                "the hooks directory {} is inside the worktree {}. Use a directory outside every worktree.",
                dir.display(),
                worktree.path.display()
            )));
        }
    }
    Ok(None)
}

/// The text of a new config file.
fn config_text(agents: &[String]) -> String {
    let defaults = Config::default();
    let list = |items: &[String]| items.iter().map(|i| format!("{i:?}")).collect::<Vec<_>>().join(", ");
    format!(
        "required_version = \"={}\"\nagents = [{}]\nlogs_dir = {:?}\nprotected = [{}]\n",
        prov_core::VERSION,
        list(agents),
        defaults.logs_dir,
        list(&defaults.protected),
    )
}

pub fn run(args: &Args) -> Result<bool> {
    let ctx = Ctx::here()?;
    let repo = &ctx.repo;
    if let Some(var) = prov_adapters::agent_env_marker() {
        bail!("`prov init` is for a human. {var} is set: an agent started this command.");
    }
    if ctx.on_agent_branch() {
        bail!("`prov init` runs on a human branch, not on the agent branch `{}`", repo.branch.as_deref().unwrap_or(""));
    }
    if let Some(problem) = hooks_dir_problem(repo)? {
        bail!("{problem}");
    }

    let mut report = Report { dry_run: args.dry_run, actions: Vec::new(), to_commit: Vec::new() };
    let write = |path: &Path, text: &str| -> Result<()> {
        if args.dry_run {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("cannot create {}", parent.display()))?;
        }
        fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))
    };

    // 1. The config file.
    let config_path = repo.toplevel.join(CONFIG_FILE);
    let existing = match fs::read_to_string(&config_path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", config_path.display())),
    };
    let config = match &existing {
        None => {
            let agents = if args.agents.is_empty() { Config::default().agents } else { args.agents.clone() };
            let text = config_text(&agents);
            let config = Config::parse(&text)?;
            write(&config_path, &text)?;
            report.actions.push(Action { target: CONFIG_FILE.into(), result: "written", detail: None });
            report.to_commit.push(CONFIG_FILE.into());
            config
        }
        Some(text) => match Config::parse(text) {
            Ok(config) if args.agents.is_empty() || args.agents == config.agents => {
                report.actions.push(Action { target: CONFIG_FILE.into(), result: "kept", detail: None });
                config
            }
            parsed => {
                if !args.force {
                    match parsed {
                        Ok(config) => bail!(
                            "{CONFIG_FILE} exists with agents = {:?}. Edit the file, or replace it with --force.",
                            config.agents
                        ),
                        Err(e) => bail!("{e:#}. Correct the file, or replace it with --force."),
                    }
                }
                let agents = if args.agents.is_empty() { Config::default().agents } else { args.agents.clone() };
                let text = config_text(&agents);
                let config = Config::parse(&text)?;
                write(&config_path, &text)?;
                report.actions.push(Action {
                    target: CONFIG_FILE.into(),
                    result: "written",
                    detail: Some("replaced the existing file".into()),
                });
                report.to_commit.push(CONFIG_FILE.into());
                config
            }
        },
    };
    if let Err(e) = config.check_version(prov_core::VERSION) {
        crate::context::say(format!("warning: {e}. The merge gate and `prov check` will refuse."));
    }

    // 2. The settings of each agent: the hooks, the identity, no signature.
    for agent in &config.agents {
        let adapter = prov_adapters::by_agent_name(agent)
            .with_context(|| format!("no adapter for the agent `{agent}` in {CONFIG_FILE}"))?;
        let identity = prov_adapters::identity(&config, agent).with_context(|| format!("no identity for `{agent}`"))?;
        let file = adapter.settings_file();
        let path = repo.toplevel.join(file);
        let existing = match fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        };
        let merged = adapter.merge_settings(existing.as_deref(), &identity)?;
        if existing.as_deref() == Some(merged.as_str()) {
            report.actions.push(Action { target: file.into(), result: "kept", detail: None });
            continue;
        }
        // A value that the file sets differently is the user's: replace it only with --force.
        let conflicts: Vec<String> = adapter
            .settings_problems(existing.as_deref(), &identity)
            .into_iter()
            .filter(|p| existing.is_some() && p.starts_with("`env.") && !p.contains("is not set"))
            .collect();
        if !conflicts.is_empty() && !args.force {
            bail!("{file} sets other values: {}. Replace them with --force.", conflicts.join("; "));
        }
        write(&path, &merged)?;
        report.actions.push(Action {
            target: file.into(),
            result: "written",
            detail: existing.is_some().then(|| "merged into the existing file; all other keys stay".to_string()),
        });
        report.to_commit.push(file.into());
    }

    // 3. The shims. An existing hook stays as `<name>.local`, and `prov hook` runs it.
    let hooks = repo.hooks_dir()?;
    for name in HOOK_NAMES {
        let path = hooks.join(name);
        let wanted = shim(name);
        let target = format!("hook {name}");
        match fs::read_to_string(&path) {
            Ok(text) if text == wanted => {
                report.actions.push(Action { target, result: "kept", detail: None });
                continue;
            }
            Ok(_) => {
                let local = hooks.join(format!("{name}.local"));
                if local.exists() {
                    if !args.force {
                        bail!(
                            "{} is not the shim of prov, and {} exists too. Remove one, or replace the hook with --force.",
                            path.display(),
                            local.display()
                        );
                    }
                    report.actions.push(Action {
                        target: target.clone(),
                        result: "written",
                        detail: Some(format!("replaced the existing hook; {name}.local stays")),
                    });
                } else {
                    if !args.dry_run {
                        fs::rename(&path, &local).with_context(|| format!("cannot move {}", path.display()))?;
                    }
                    report.actions.push(Action {
                        target: target.clone(),
                        result: "moved",
                        detail: Some(format!("the existing hook is now {name}.local; prov runs it after its own work")),
                    });
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                report.actions.push(Action { target, result: "written", detail: None });
            }
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
        write(&path, &wanted)?;
        if !args.dry_run {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .with_context(|| format!("cannot make {} executable", path.display()))?;
        }
    }

    // 4. The git options.
    for (key, value) in GIT_OPTIONS {
        // Only the value in the repository counts: a clone must not depend on one user's settings.
        let local = repo.git_opt(["config", "--local", "--get", key])?;
        if local.as_deref() == Some(value) {
            report.actions.push(Action { target: format!("git config {key}"), result: "kept", detail: None });
        } else {
            if !args.dry_run {
                repo.set_config_value(key, value)?;
            }
            report.actions.push(Action { target: format!("git config {key}"), result: "set", detail: Some(value.into()) });
        }
    }

    // 5. `.gitignore`, if the logs directory is inside this worktree.
    if let Some(line) = ignore_line(repo, &config)? {
        let path = repo.toplevel.join(".gitignore");
        let mut text = fs::read_to_string(&path).unwrap_or_default();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&line);
        text.push('\n');
        write(&path, &text)?;
        report.actions.push(Action { target: ".gitignore".into(), result: "written", detail: Some(format!("added {line}")) });
        report.to_commit.push(".gitignore".into());
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for a in &report.actions {
            match &a.detail {
                Some(detail) => println!("{:8} {} ({detail})", a.result, a.target),
                None => println!("{:8} {}", a.result, a.target),
            }
        }
        if args.dry_run {
            println!("Dry run: nothing changed.");
        } else if !report.to_commit.is_empty() {
            println!("Commit these files on this branch: {}", report.to_commit.join(" "));
            println!("Then check the setup: prov doctor");
        }
    }
    Ok(true)
}

/// The `.gitignore` line for the logs directory, if the directory is inside
/// this worktree and git does not ignore it yet.
fn ignore_line(repo: &Repo, config: &Config) -> Result<Option<String>> {
    let logs: PathBuf = repo.base_dir().join(&config.logs_dir);
    let Ok(rel) = logs.strip_prefix(&repo.toplevel) else { return Ok(None) };
    let Some(rel) = rel.to_str().filter(|r| !r.is_empty()) else { return Ok(None) };
    let rel = rel.trim_end_matches('/');
    if repo.git_ok(["check-ignore", "-q", &format!("{rel}/probe")])? {
        return Ok(None);
    }
    Ok(Some(format!("/{rel}/")))
}
