//! Repository and worktree discovery, and the git commands that `prov` runs.
//!
//! `prov` changes git state only through the `git` command.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use prov_core::config::CONFIG_FILE;
use prov_core::trailer::{self, Trailer};
use prov_core::Config;

/// The git hooks that get a shim.
pub const HOOK_NAMES: [&str; 4] = ["pre-commit", "prepare-commit-msg", "post-commit", "post-merge"];

/// The content of the shim for a git hook. POSIX `sh`.
pub fn shim(name: &str) -> String {
    format!("#!/bin/sh\nexec prov hook git {name} \"$@\"\n")
}

/// One worktree of a repository.
#[derive(Debug, Clone)]
pub struct Repo {
    /// The root of this worktree.
    pub toplevel: PathBuf,
    /// The git directory of this worktree.
    pub git_dir: PathBuf,
    /// The common git directory, which all worktrees share.
    pub common_dir: PathBuf,
    /// The checked-out branch. `None` with a detached `HEAD`.
    pub branch: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub sha: String,
    pub author_email: String,
    pub parents: Vec<String>,
    pub subject: String,
    pub trailers: Vec<Trailer>,
}

impl CommitInfo {
    pub fn short(&self) -> &str {
        &self.sha[..self.sha.len().min(10)]
    }
}

impl Repo {
    /// Finds the worktree that contains `dir`. `None` if `dir` is not in a worktree.
    pub fn discover(dir: &Path) -> Result<Option<Repo>> {
        let out = Command::new("git")
            .current_dir(dir)
            .args(["rev-parse", "--path-format=absolute", "--show-toplevel", "--git-dir", "--git-common-dir"])
            .stderr(Stdio::null())
            .output()
            .context("cannot run git")?;
        if !out.status.success() {
            return Ok(None);
        }
        let text = String::from_utf8(out.stdout).context("git printed a path that is not UTF-8")?;
        let mut lines = text.lines();
        let (Some(toplevel), Some(git_dir), Some(common_dir)) = (lines.next(), lines.next(), lines.next()) else {
            return Ok(None);
        };
        let mut repo = Repo {
            toplevel: PathBuf::from(toplevel),
            git_dir: PathBuf::from(git_dir),
            common_dir: PathBuf::from(common_dir),
            branch: None,
        };
        repo.branch = repo.git_opt(["symbolic-ref", "-q", "--short", "HEAD"])?;
        Ok(Some(repo))
    }

    /// The directory that holds the common git directory.
    pub fn base_dir(&self) -> &Path {
        self.common_dir.parent().unwrap_or(&self.common_dir)
    }

    fn command<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new("git");
        cmd.current_dir(&self.toplevel).args(args).stdin(Stdio::null());
        cmd
    }

    /// Runs git and returns its standard output, without the last line break.
    pub fn git<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = self.command(args);
        let out = cmd.output().context("cannot run git")?;
        if !out.status.success() {
            bail!("{:?} failed: {}", cmd.get_args().collect::<Vec<_>>(), String::from_utf8_lossy(&out.stderr).trim());
        }
        let mut text = String::from_utf8(out.stdout).context("git output is not UTF-8")?;
        while text.ends_with('\n') {
            text.pop();
        }
        Ok(text)
    }

    /// As [`Repo::git`], but `None` when git exits with an error.
    pub fn git_opt<I, S>(&self, args: I) -> Result<Option<String>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let out = self.command(args).stderr(Stdio::null()).output().context("cannot run git")?;
        if !out.status.success() {
            return Ok(None);
        }
        let mut text = String::from_utf8(out.stdout).context("git output is not UTF-8")?;
        while text.ends_with('\n') {
            text.pop();
        }
        Ok(Some(text))
    }

    pub fn git_ok<I, S>(&self, args: I) -> Result<bool>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Ok(self.command(args).stdout(Stdio::null()).stderr(Stdio::null()).status().context("cannot run git")?.success())
    }

    /// The full SHA of a commit, or `None` if the revision does not exist.
    pub fn rev_parse(&self, rev: &str) -> Result<Option<String>> {
        self.git_opt(["rev-parse", "-q", "--verify", &format!("{rev}^{{commit}}")])
    }

    /// A path inside the git directory of this worktree, for example `MERGE_HEAD`.
    pub fn git_path(&self, name: &str) -> Result<PathBuf> {
        Ok(PathBuf::from(self.git(["rev-parse", "--path-format=absolute", "--git-path", name])?))
    }

    /// The commits of a merge in progress. Empty when there is none.
    pub fn merge_heads(&self) -> Result<Vec<String>> {
        match std::fs::read_to_string(self.git_path("MERGE_HEAD")?) {
            Ok(text) => Ok(text.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e).context("cannot read MERGE_HEAD"),
        }
    }

    pub fn show_file(&self, rev: &str, path: &str) -> Result<Option<String>> {
        self.git_opt(["show", &format!("{rev}:{path}")])
    }

    /// The config at a commit. `None` if the commit has no config file.
    pub fn config_at(&self, rev: &str) -> Result<Option<Config>> {
        match self.show_file(rev, CONFIG_FILE)? {
            Some(text) => Ok(Some(Config::parse(&text).with_context(|| format!("at {rev}"))?)),
            None => Ok(None),
        }
    }

    /// The config of this checkout. The defaults if the file does not exist.
    pub fn config_here(&self) -> Result<Config> {
        match std::fs::read_to_string(self.toplevel.join(CONFIG_FILE)) {
            Ok(text) => Config::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| format!("cannot read {CONFIG_FILE}")),
        }
    }

    /// Commits selected by `git log` arguments, in the order git prints them.
    pub fn commits<I, S>(&self, args: I) -> Result<Vec<CommitInfo>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd_args: Vec<std::ffi::OsString> =
            vec!["log".into(), "--format=%H%x1f%ae%x1f%P%x1f%s%x1f%(trailers:only,unfold)%x1e".into()];
        cmd_args.extend(args.into_iter().map(|a| a.as_ref().to_os_string()));
        let text = self.git(cmd_args)?;
        let mut out = Vec::new();
        for record in text.split('\x1e') {
            let record = record.trim_start_matches('\n');
            if record.is_empty() {
                continue;
            }
            let fields: Vec<&str> = record.splitn(5, '\x1f').collect();
            if fields.len() != 5 {
                bail!("unexpected output of git log");
            }
            out.push(CommitInfo {
                sha: fields[0].to_string(),
                author_email: fields[1].to_string(),
                parents: fields[2].split_whitespace().map(String::from).collect(),
                subject: fields[3].to_string(),
                trailers: trailer::parse_block(fields[4]),
            });
        }
        Ok(out)
    }

    pub fn commit(&self, rev: &str) -> Result<CommitInfo> {
        self.commits(["--no-walk", rev])?.into_iter().next().with_context(|| format!("no commit {rev}"))
    }

    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        self.git_ok(["merge-base", "--is-ancestor", ancestor, descendant])
    }

    /// The paths that differ between the merge base of `base` and `tip`, and `tip`.
    /// The paths that a merge commit changed relative to every parent: what
    /// the author of the merge wrote, for example to resolve a conflict.
    /// Empty for a clean merge.
    pub fn merge_resolved_paths(&self, commit: &str) -> Result<Vec<String>> {
        let text = self.git(["diff-tree", "--no-commit-id", "--name-only", "-r", "-c", "-z", commit])?;
        Ok(text.split('\0').filter(|p| !p.is_empty()).map(String::from).collect())
    }

    pub fn changed_paths(&self, base: &str, tip: &str) -> Result<Vec<String>> {
        let text = self.git(["diff", "--name-only", "--no-renames", "-z", &format!("{base}...{tip}")])?;
        Ok(text.split('\0').filter(|p| !p.is_empty()).map(String::from).collect())
    }

    /// The files of the worktree that differ from `HEAD` and still exist:
    /// changed, staged and untracked files. Absolute paths.
    pub fn changed_files(&self) -> Result<Vec<PathBuf>> {
        let text = self.git(["status", "--porcelain", "-z", "--untracked-files=all", "--no-renames"])?;
        let mut out = Vec::new();
        for entry in text.split('\0') {
            // `XY <path>`
            let Some(path) = entry.get(3..) else { continue };
            if path.is_empty() {
                continue;
            }
            let abs = self.toplevel.join(path);
            if abs.is_file() || abs.is_symlink() {
                out.push(abs);
            }
        }
        Ok(out)
    }

    /// The author email that a commit made now would get.
    pub fn author_email(&self) -> Result<Option<String>> {
        let Some(ident) = self.git_opt(["var", "GIT_AUTHOR_IDENT"])? else { return Ok(None) };
        Ok(ident.split_once('<').and_then(|(_, rest)| rest.split_once('>')).map(|(email, _)| email.to_string()))
    }

    /// The trailers of a commit message file.
    pub fn message_trailers(&self, file: &Path) -> Result<Vec<Trailer>> {
        let text = self.git([OsStr::new("interpret-trailers"), OsStr::new("--parse"), file.as_os_str()])?;
        Ok(trailer::parse_block(&text))
    }

    /// Adds trailers to a commit message file, in the given order.
    pub fn add_trailers(&self, file: &Path, trailers: &[Trailer]) -> Result<()> {
        if trailers.is_empty() {
            return Ok(());
        }
        let mut args: Vec<std::ffi::OsString> =
            vec!["interpret-trailers".into(), "--in-place".into(), "--if-exists".into(), "add".into()];
        for t in trailers {
            args.push("--trailer".into());
            args.push(t.to_string().into());
        }
        args.push(file.as_os_str().to_os_string());
        self.git(args)?;
        Ok(())
    }
}
