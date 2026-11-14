//! Repository and worktree discovery, and the git commands that `prov` runs.
//!
//! `prov` changes git state only through the `git` command.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use prov_messages as msg;
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

/// One entry of `git worktree list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// `None` with a detached `HEAD`.
    pub branch: Option<String>,
}

/// Removes `.` and `..` components without reading the file system.
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The branch in the `HEAD` file of a git directory: `Some(None)` for a
/// detached `HEAD`, and `None` if the file does not answer the question.
///
/// Every command of `prov` needs the branch. Reading the file saves one git
/// process, which matters in a hook on a slow file system: there, each git
/// process costs tens of milliseconds.
#[allow(clippy::option_option)] // three cases, as the comment says
fn head_branch(git_dir: &Path) -> Option<Option<String>> {
    let text = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let text = text.trim_end();
    match text.strip_prefix("ref: ") {
        Some(target) => {
            let branch = target.trim().strip_prefix("refs/heads/")?;
            // With the reftable format, the file holds a placeholder.
            (!branch.is_empty() && branch != ".invalid").then(|| Some(branch.to_string()))
        }
        None => (text.len() >= 40 && text.bytes().all(|b| b.is_ascii_hexdigit())).then_some(None),
    }
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
        // A directory that is gone, for example a removed worktree in which
        // an agent still runs, is in no repository.
        if !dir.is_dir() {
            return Ok(None);
        }
        let out = Command::new("git")
            .current_dir(dir)
            .args(["rev-parse", "--path-format=absolute", "--show-toplevel", "--git-dir", "--git-common-dir"])
            .stderr(Stdio::null())
            .output()
            .context(msg::run::CANNOT_RUN_GIT)?;
        if !out.status.success() {
            return Ok(None);
        }
        let text = String::from_utf8(out.stdout).context(msg::git::PATH_NOT_UTF8)?;
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
        repo.branch = match head_branch(&repo.git_dir) {
            Some(branch) => branch,
            None => repo.git_opt(["symbolic-ref", "-q", "--short", "HEAD"])?,
        };
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
        let out = cmd.output().context(msg::run::CANNOT_RUN_GIT)?;
        if !out.status.success() {
            bail!(msg::git::failed(cmd.get_args().collect::<Vec<_>>(), String::from_utf8_lossy(&out.stderr).trim()));
        }
        let mut text = String::from_utf8(out.stdout).context(msg::git::OUTPUT_NOT_UTF8)?;
        while text.ends_with('\n') {
            text.pop();
        }
        Ok(text)
    }

    /// As [`Repo::git`], but `None` when git exits with an error.
    /// The output of git as bytes: the content of a file can be any bytes.
    pub fn git_bytes<I, S>(&self, args: I) -> Result<Vec<u8>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = self.command(args);
        let out = cmd.output().context(msg::run::CANNOT_RUN_GIT)?;
        if !out.status.success() {
            bail!(msg::git::failed(cmd.get_args().collect::<Vec<_>>(), String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(out.stdout)
    }

    /// The blob ID of each file of a commit, by path: one `git ls-tree`.
    pub fn tree_blobs(&self, commit: &str) -> Result<std::collections::HashMap<String, String>> {
        let text = self.git(["ls-tree", "-r", "-z", "--full-tree", commit])?;
        Ok(text
            .split('\0')
            .filter_map(|entry| {
                // `<mode> <type> <id>\t<path>`
                let (meta, path) = entry.split_once('\t')?;
                let id = meta.split(' ').nth(2)?;
                Some((path.to_string(), id.to_string()))
            })
            .collect())
    }

    /// The content of each blob, by ID: one `git cat-file --batch`.
    pub fn blobs(&self, ids: &[&str]) -> Result<std::collections::HashMap<String, Vec<u8>>> {
        use std::io::Write;
        let mut cmd = self.command(["cat-file", "--batch"]);
        cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().context(msg::run::CANNOT_RUN_GIT)?;
        let mut stdin = child.stdin.take().context(msg::run::CANNOT_RUN_GIT)?;
        let mut input = ids.join("\n");
        input.push('\n');
        // A thread writes, so that a large output cannot block the write.
        let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
        let out = child.wait_with_output().context(msg::run::CANNOT_RUN_GIT)?;
        writer.join().map_err(|_| anyhow::anyhow!(msg::run::CANNOT_RUN_GIT))?.context(msg::run::CANNOT_RUN_GIT)?;
        if !out.status.success() {
            bail!(msg::git::failed(["cat-file", "--batch"], String::from_utf8_lossy(&out.stderr).trim()));
        }
        // `<id> blob <size>\n<content>\n` for each ID, or `<id> missing\n`.
        let mut blobs = std::collections::HashMap::new();
        let mut rest = out.stdout.as_slice();
        while let Some(end) = rest.iter().position(|b| *b == b'\n') {
            let header = String::from_utf8_lossy(&rest[..end]).into_owned();
            rest = &rest[end + 1..];
            let mut words = header.split(' ');
            let (Some(id), Some(kind), Some(size)) = (words.next(), words.next(), words.next()) else { continue };
            let Ok(size) = size.parse::<usize>() else { continue };
            if rest.len() < size + 1 {
                bail!(msg::git::failed(["cat-file", "--batch"], msg::git::CUT_OFF));
            }
            if kind == "blob" {
                blobs.insert(id.to_string(), rest[..size].to_vec());
            }
            rest = &rest[size + 1..];
        }
        Ok(blobs)
    }

    pub fn git_opt<I, S>(&self, args: I) -> Result<Option<String>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let out = self.command(args).stderr(Stdio::null()).output().context(msg::run::CANNOT_RUN_GIT)?;
        if !out.status.success() {
            return Ok(None);
        }
        let mut text = String::from_utf8(out.stdout).context(msg::git::OUTPUT_NOT_UTF8)?;
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
        Ok(self.command(args).stdout(Stdio::null()).stderr(Stdio::null()).status().context(msg::run::CANNOT_RUN_GIT)?.success())
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
            Err(e) => Err(e).context(msg::git::CANNOT_READ_MERGE_HEAD),
        }
    }

    pub fn show_file(&self, rev: &str, path: &str) -> Result<Option<String>> {
        self.git_opt(["show", &format!("{rev}:{path}")])
    }

    /// The config at a commit. `None` if the commit has no config file.
    pub fn config_at(&self, rev: &str) -> Result<Option<Config>> {
        match self.show_file(rev, CONFIG_FILE)? {
            Some(text) => Ok(Some(Config::parse(&text).with_context(|| msg::git::at_rev(rev))?)),
            None => Ok(None),
        }
    }

    /// The config of this checkout. The defaults if the file does not exist.
    pub fn config_here(&self) -> Result<Config> {
        match std::fs::read_to_string(self.toplevel.join(CONFIG_FILE)) {
            Ok(text) => Config::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| msg::fs::cannot_read(CONFIG_FILE)),
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
                bail!(msg::git::UNEXPECTED_LOG);
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
        self.commits(["--no-walk", rev])?.into_iter().next().with_context(|| msg::git::no_commit(rev))
    }

    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        self.git_ok(["merge-base", "--is-ancestor", ancestor, descendant])
    }

    /// The paths that a merge commit changed relative to every parent: what
    /// the author of the merge wrote, for example to resolve a conflict.
    /// Empty for a clean merge.
    pub fn merge_resolved_paths(&self, commit: &str) -> Result<Vec<String>> {
        let text = self.git(["diff-tree", "--no-commit-id", "--name-only", "-r", "-c", "-z", commit])?;
        Ok(text.split('\0').filter(|p| !p.is_empty()).map(String::from).collect())
    }

    /// The paths that a commit changes, each with the kind of change, by one
    /// `git diff-tree --raw --numstat`. Not for a merge commit.
    pub fn path_changes(&self, commit: &str) -> Result<Vec<(String, PathChange)>> {
        let text = self.git([
            "diff-tree", "--no-commit-id", "--raw", "--numstat", "--no-renames", "-r", "--root", "-z", commit,
        ])?;
        Ok(parse_path_changes(&text))
    }

    /// The paths that differ between the merge base of `base` and `tip`, and `tip`.
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

    /// The hooks directory: `core.hooksPath`, or `hooks` in the common git directory.
    pub fn hooks_dir(&self) -> Result<PathBuf> {
        self.git_path("hooks")
    }

    /// A value of the git configuration, from every level. `None` if it is not set.
    pub fn config_value(&self, key: &str) -> Result<Option<String>> {
        self.git_opt(["config", "--get", key])
    }

    /// Sets a value in the configuration of the repository.
    pub fn set_config_value(&self, key: &str, value: &str) -> Result<()> {
        self.git(["config", "--local", key, value]).map(|_| ())
    }

    /// Every worktree of the repository, the main worktree first.
    pub fn worktrees(&self) -> Result<Vec<Worktree>> {
        let text = self.git(["worktree", "list", "--porcelain", "-z"])?;
        let mut out: Vec<Worktree> = Vec::new();
        for field in text.split('\0') {
            if let Some(path) = field.strip_prefix("worktree ") {
                out.push(Worktree { path: PathBuf::from(path), branch: None });
            } else if let (Some(branch), Some(last)) = (field.strip_prefix("branch refs/heads/"), out.last_mut()) {
                last.branch = Some(branch.to_string());
            }
        }
        Ok(out)
    }

    /// The path of a file relative to the root of the worktree, with `/`.
    /// `path` is relative to the current directory, or absolute.
    pub fn relative_path(&self, path: &Path) -> Result<String> {
        let abs = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir()?.join(path) };
        let abs = normalize(&abs);
        let top = std::fs::canonicalize(&self.toplevel).unwrap_or_else(|_| self.toplevel.clone());
        // The parent directory resolves symbolic links; the file itself can be one.
        let resolved = match (abs.parent().and_then(|p| std::fs::canonicalize(p).ok()), abs.file_name()) {
            (Some(parent), Some(name)) => parent.join(name),
            _ => abs.clone(),
        };
        let rel = resolved
            .strip_prefix(&top)
            .or_else(|_| abs.strip_prefix(&self.toplevel))
            .with_context(|| msg::git::not_in_worktree(path.display(), self.toplevel.display()))?;
        let text = rel.to_str().with_context(|| msg::git::not_utf8(path.display()))?;
        if text.is_empty() {
            bail!(msg::git::worktree_root(path.display()));
        }
        Ok(text.to_string())
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

/// How a commit changes a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathChange {
    /// The commit deletes the file.
    Deleted,
    /// The commit adds lines, changes a binary file, or changes the mode or
    /// the type of the file.
    Content,
    /// The commit only deletes lines of the file, or does not change its
    /// content at all.
    LinesDeleted,
}

/// Parses `git diff-tree --raw --numstat -z`: first a raw record for each
/// path (`:<old mode> <new mode> <old id> <new id> <status>`, then the
/// path), then a numstat record for each path (`<added>\t<deleted>\t<path>`,
/// `-` for a binary file).
fn parse_path_changes(text: &str) -> Vec<(String, PathChange)> {
    let mut out: Vec<(String, PathChange)> = Vec::new();
    let mut records = text.split('\0').filter(|r| !r.is_empty());
    while let Some(record) = records.next() {
        if let Some(raw) = record.strip_prefix(':') {
            let Some(path) = records.next() else { break };
            let fields: Vec<&str> = raw.split(' ').collect();
            let change = match fields.as_slice() {
                [_, _, _, _, status] if status.starts_with('D') => PathChange::Deleted,
                [old_mode, new_mode, ..] if old_mode != new_mode && *old_mode != "000000" => PathChange::Content,
                [_, _, _, _, status] if status.starts_with('T') => PathChange::Content,
                _ => PathChange::LinesDeleted,
            };
            out.push((path.to_string(), change));
        } else {
            let mut parts = record.splitn(3, '\t');
            let (Some(added), Some(_), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue };
            let content = added == "-" || added.parse::<u64>().is_ok_and(|n| n > 0);
            if let Some(entry) = out.iter_mut().find(|(p, _)| p == path) {
                if content && entry.1 == PathChange::LinesDeleted {
                    entry.1 = PathChange::Content;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Output of git 2.53 for a commit that changes a binary file, deletes
    /// a file, makes a script executable, adds a file with a space in its
    /// name and deletes a line of a text file.
    #[test]
    fn reads_path_changes() {
        let raw = [
            ":100644 100644 d5d0 5d9e M", "b.bin",
            ":100644 000000 1191 0000 D", "g.txt",
            ":100644 100755 fa11 fa11 M", "s.sh",
            ":000000 100644 0000 8ba3 A", "sp ace.txt",
            ":100644 100644 422c 7898 M", "t.txt",
            "-\t-\tb.bin", "0\t2\tg.txt", "0\t0\ts.sh", "1\t0\tsp ace.txt", "0\t1\tt.txt", "",
        ]
        .join("\0");
        assert_eq!(
            parse_path_changes(&raw),
            vec![
                ("b.bin".to_string(), PathChange::Content),
                ("g.txt".to_string(), PathChange::Deleted),
                ("s.sh".to_string(), PathChange::Content),
                ("sp ace.txt".to_string(), PathChange::Content),
                ("t.txt".to_string(), PathChange::LinesDeleted),
            ]
        );
    }

    #[test]
    fn reads_the_branch_from_the_head_file() {
        let dir = std::env::temp_dir().join(format!("prov-head-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let head = |text: &str| {
            std::fs::write(dir.join("HEAD"), text).unwrap();
            head_branch(&dir)
        };
        assert_eq!(head("ref: refs/heads/agent/claude/x\n"), Some(Some("agent/claude/x".into())));
        assert_eq!(head(&format!("{}\n", "a".repeat(40))), Some(None));
        // Not answered here: git decides.
        assert_eq!(head("ref: refs/heads/.invalid\n"), None);
        assert_eq!(head("ref: refs/tags/v1\n"), None);
        assert_eq!(head("garbage\n"), None);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(head_branch(&dir), None);
    }
}
