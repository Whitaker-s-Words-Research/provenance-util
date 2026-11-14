//! `prov revise [--of <merge>]`: commits a human's staged changes to the
//! lines of an integration, with a `Prov-Revises` trailer.
//!
//! R-REV-1: a line counts when the staged diff changes or deletes it. A
//! counted line is a merged line when `git blame HEAD` gives it to the
//! target or to a commit in `<target>^1..<target>^2`. The commit is refused
//! if no counted line is a merged line.

use std::collections::{BTreeMap, HashSet};
use std::process::Command;

use anyhow::{bail, Context, Result};
use prov_core::trailer;
use prov_git::{CommitInfo, Repo};
use prov_messages as msg;

use crate::context::Ctx;

pub struct Args {
    pub of: Option<String>,
    pub dry_run: bool,
}

/// The lines of `HEAD` that the staged diff changes or deletes, by path at `HEAD`.
type Counted = BTreeMap<String, Vec<(u32, u32)>>;

pub fn run(args: &Args) -> Result<bool> {
    let ctx = Ctx::here()?;
    let repo = &ctx.repo;
    if let Some(var) = prov_adapters::agent_env_marker() {
        bail!(msg::revise::agent_env(var));
    }
    if ctx.on_agent_branch() {
        bail!(msg::revise::AGENT_WORKTREE);
    }

    let target = target(repo, args.of.as_deref())?;
    let since = repo.git(["rev-list", "--count", &format!("{}..HEAD", target.sha)])?;
    println!("{}", msg::revise::target(target.short(), &target.subject, since));

    staged_only(repo)?;
    let counted = counted_lines(repo)?;
    let merged_commits = merged_commits(repo, &target)?;
    let (merged, other) = classify(repo, &counted, &merged_commits)?;
    println!("{}", msg::revise::counts(merged, other));
    if merged == 0 {
        bail!(msg::revise::NO_MERGED_LINE);
    }
    if args.dry_run {
        println!("{}", msg::revise::DRY_RUN);
        return Ok(true);
    }
    commit(repo, &target, merged, other)
}

/// An integration: a merge with a `Prov-Session` trailer.
fn is_integration(commit: &CommitInfo) -> bool {
    commit.parents.len() == 2 && trailer::has(&commit.trailers, trailer::PROV_SESSION)
}

/// The target of the revision: `--of`, or the first integration on the
/// first-parent line from `HEAD`.
fn target(repo: &Repo, of: Option<&str>) -> Result<CommitInfo> {
    if let Some(rev) = of {
        let sha = repo.rev_parse(rev)?.with_context(|| msg::revise::no_commit(rev))?;
        let commit = repo.commit(&sha)?;
        if !is_integration(&commit) {
            bail!(msg::revise::not_integration(rev));
        }
        if !repo.is_ancestor(&sha, "HEAD")? {
            bail!(msg::revise::not_ancestor(rev));
        }
        return Ok(commit);
    }
    // `--grep` only narrows the walk; the trailers decide.
    let candidates = repo.commits(["--first-parent", "--merges", "-E", "--grep=^Prov-Session: ", "HEAD"])?;
    candidates.into_iter().find(is_integration).context(msg::revise::NO_INTEGRATION)
}

/// Refuses an empty index, and a file with staged changes that also has
/// changes that are not staged: git-ai records the file in the working
/// tree, not the staged content.
fn staged_only(repo: &Repo) -> Result<()> {
    let staged = repo.git(["diff", "--cached", "--name-only", "--no-renames", "-z"])?;
    let staged: HashSet<&str> = staged.split('\0').filter(|p| !p.is_empty()).collect();
    if staged.is_empty() {
        bail!(msg::revise::NOTHING_STAGED);
    }
    let unstaged = repo.git(["diff", "--name-only", "--no-renames", "-z"])?;
    let mut both: Vec<&str> = unstaged.split('\0').filter(|p| staged.contains(p)).collect();
    if !both.is_empty() {
        both.sort_unstable();
        bail!(msg::revise::partly_staged(both.join(", ")));
    }
    Ok(())
}

/// The old path of each changed file, in the order of `git diff`. `None` for
/// a file with no old lines to blame: an added file or a submodule.
fn old_paths(repo: &Repo) -> Result<Vec<Option<String>>> {
    let raw = repo.git(["diff", "--cached", "-M", "--raw", "-z", "HEAD"])?;
    let mut fields = raw.split('\0');
    let mut out = Vec::new();
    while let Some(meta) = fields.next() {
        if meta.is_empty() {
            continue;
        }
        // `:<old mode> <new mode> <old id> <new id> <status>`, then the
        // path, or two paths for a rename or a copy.
        let parts: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
        let [old_mode, _, _, _, status] = parts.as_slice() else { bail!(msg::revise::UNEXPECTED_DIFF) };
        let old = fields.next().context(msg::revise::UNEXPECTED_DIFF)?;
        if status.starts_with('R') || status.starts_with('C') {
            fields.next().context(msg::revise::UNEXPECTED_DIFF)?;
        }
        let blameable = *old_mode != "000000" && *old_mode != "160000";
        out.push(blameable.then(|| old.to_string()));
    }
    Ok(out)
}

/// `@@ -<start>[,<count>] +... @@`: the old lines of a hunk, if any.
fn old_range(hunk: &str) -> Option<(u32, u32)> {
    let old = hunk.strip_prefix("@@ -")?.split(' ').next()?;
    let (start, count) = match old.split_once(',') {
        Some((s, c)) => (s.parse().ok()?, c.parse().ok()?),
        None => (old.parse().ok()?, 1),
    };
    (count > 0).then(|| (start, start + count - 1))
}

/// R-REV-1: the lines on the old side of each hunk of the staged diff. A
/// line that the diff only adds does not count.
fn counted_lines(repo: &Repo) -> Result<Counted> {
    let paths = old_paths(repo)?;
    let patch = repo.git(["diff", "--cached", "-M", "-U0", "--no-color", "--no-ext-diff", "HEAD"])?;
    let mut counted = Counted::new();
    let mut file: Option<usize> = None;
    for line in patch.lines() {
        if line.starts_with("diff --git ") {
            file = Some(file.map_or(0, |i| i + 1));
            continue;
        }
        let (Some(i), Some(range)) = (file, old_range(line)) else { continue };
        if let Some(old) = paths.get(i).context(msg::revise::UNEXPECTED_DIFF)? {
            counted.entry(old.clone()).or_default().push(range);
        }
    }
    Ok(counted)
}

/// The commits whose lines came in with the target: the target itself (a
/// conflict resolution) and `<target>^1..<target>^2`.
fn merged_commits(repo: &Repo, target: &CommitInfo) -> Result<HashSet<String>> {
    let [first, second] = target.parents.as_slice() else { bail!(msg::revise::not_integration(&target.sha)) };
    let list = repo.git(["rev-list", &format!("{first}..{second}")])?;
    let mut set: HashSet<String> = list.lines().map(String::from).collect();
    set.insert(target.sha.clone());
    Ok(set)
}

/// One `git blame` call for each file. Returns the number of merged lines
/// and of other counted lines.
fn classify(repo: &Repo, counted: &Counted, merged_commits: &HashSet<String>) -> Result<(usize, usize)> {
    let (mut merged, mut other) = (0, 0);
    for (path, ranges) in counted {
        let mut args: Vec<String> = vec!["blame".into(), "--porcelain".into(), "HEAD".into()];
        for (start, end) in ranges {
            args.push("-L".into());
            args.push(format!("{start},{end}"));
        }
        args.push("--".into());
        args.push(path.clone());
        for commit in blamed_commits(&repo.git(&args)?) {
            if merged_commits.contains(commit) {
                merged += 1;
            } else {
                other += 1;
            }
        }
    }
    Ok((merged, other))
}

/// The commit of each line in `git blame --porcelain` output. Each line has
/// one header: `<commit> <old line> <new line>[ <count>]`. Content lines
/// start with a tab, and no metadata line starts with a hex word.
fn blamed_commits(porcelain: &str) -> Vec<&str> {
    porcelain
        .lines()
        .filter_map(|line| {
            let mut words = line.split(' ');
            let commit = words.next()?;
            let is_header = commit.len() >= 40
                && commit.bytes().all(|b| b.is_ascii_hexdigit())
                && words.next().is_some_and(|w| w.parse::<u32>().is_ok());
            is_header.then_some(commit)
        })
        .collect()
}

/// Starts `git commit` with a template: an empty subject line, the trailer
/// and comment lines. git refuses the commit if the message is not changed.
fn commit(repo: &Repo, target: &CommitInfo, merged: usize, other: usize) -> Result<bool> {
    let comment = match repo.config_value("core.commentChar")? {
        None => Some("#".to_string()),
        // With `auto`, git picks a character that no line of the template
        // starts with, so comment lines would stay in the message.
        Some(c) if c == "auto" => None,
        Some(c) => Some(c),
    };
    let mut text = format!("\n\n{}: {}\n", trailer::PROV_REVISES, target.sha);
    if let Some(c) = comment {
        text.push('\n');
        for line in msg::revise::template_comment(target.short(), &target.subject, merged, other).lines() {
            text.push_str(&c);
            text.push(' ');
            text.push_str(line);
            text.push('\n');
        }
    }
    let dir = repo.common_dir.join("provenance");
    std::fs::create_dir_all(&dir).with_context(|| msg::fs::cannot_create(dir.display()))?;
    let file = dir.join(format!("revise-{}.txt", std::process::id()));
    std::fs::write(&file, text).with_context(|| msg::fs::cannot_write(file.display()))?;
    let status = Command::new("git")
        .current_dir(&repo.toplevel)
        .arg("commit")
        .arg("--template")
        .arg(&file)
        .status()
        .context(msg::run::CANNOT_RUN_GIT);
    let _ = std::fs::remove_file(&file);
    Ok(status?.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_ranges() {
        assert_eq!(old_range("@@ -3 +3 @@"), Some((3, 3)));
        assert_eq!(old_range("@@ -3,2 +3 @@ fn x"), Some((3, 4)));
        assert_eq!(old_range("@@ -3,0 +4,2 @@"), None, "only added");
        assert_eq!(old_range("@@ -1,4 +0,0 @@"), Some((1, 4)), "deleted file");
        assert_eq!(old_range("+@@ -1 +1 @@"), None);
    }

    #[test]
    fn porcelain_headers() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let text = format!(
            "{a} 3 3 2\nauthor X\nsummary 1234567890123456789012345678901234567890 9 9\n\tline\n{a} 4 4\n\tline\n{b} 7 8 1\nprevious {a} f\n\t{b} 1 1\n"
        );
        assert_eq!(blamed_commits(&text), [a.as_str(), a.as_str(), b.as_str()]);
    }
}
