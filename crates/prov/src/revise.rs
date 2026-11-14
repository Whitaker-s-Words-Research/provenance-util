//! `prov revise [--of <merge>]...`: commits a human's staged changes to the
//! lines of one or more integrations, with a `Prov-Revises` trailer for each.
//!
//! R-REV-1: a line counts when the staged diff changes or deletes it. Lines
//! that the diff inserts count when a neighbour line in `HEAD` is a merged
//! line. A line of `HEAD` is a merged line of a target when `git blame HEAD`
//! gives it to the target, to a commit in `<target>^1..<target>^2`, or to an
//! earlier revision of the target. Each target needs at least one counted
//! merged line.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::process::Command;

use anyhow::{bail, Context, Result};
use prov_core::trailer;
use prov_git::{CommitInfo, Repo};
use prov_messages as msg;

use crate::context::Ctx;

pub struct Args {
    pub of: Vec<String>,
    pub dry_run: bool,
}

/// The staged diff against `HEAD` for one file, by the path at `HEAD`.
#[derive(Debug, Default, PartialEq, Eq)]
struct FileHunks {
    /// The lines of `HEAD` that the diff changes or deletes.
    changed: Vec<(u32, u32)>,
    /// Lines that the diff only inserts: the line of `HEAD` after which they
    /// go (0 at the start of the file), and their number.
    inserted: Vec<(u32, u32)>,
}

type Diff = BTreeMap<String, FileHunks>;

/// A target and the commits whose lines came in with it.
struct Target {
    commit: CommitInfo,
    merged: HashSet<String>,
}

/// The counts of one target.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Counts {
    changed: usize,
    inserted: usize,
}

pub fn run(args: &Args) -> Result<bool> {
    let ctx = Ctx::here()?;
    let repo = &ctx.repo;
    if let Some(var) = prov_adapters::agent_env_marker() {
        bail!(msg::revise::agent_env(var));
    }
    if ctx.on_agent_branch() {
        bail!(msg::revise::AGENT_WORKTREE);
    }

    let mut targets = Vec::new();
    for commit in target_commits(repo, &args.of)? {
        let since = repo.git(["rev-list", "--count", &format!("{}..HEAD", commit.sha)])?;
        println!("{}", msg::revise::target(commit.short(), &commit.subject, since));
        let merged = merged_commits(repo, &commit)?;
        targets.push(Target { commit, merged });
    }

    staged_only(repo)?;
    let diff = staged_diff(repo)?;
    let (counts, other) = classify(repo, &diff, &targets)?;
    for (t, c) in targets.iter().zip(&counts) {
        println!("{}", msg::revise::counts(t.commit.short(), c.changed, c.inserted));
    }
    println!("{}", msg::revise::other_count(other));
    if let Some((t, _)) = targets.iter().zip(&counts).find(|(_, c)| c.changed + c.inserted == 0) {
        bail!(msg::revise::no_merged_line(t.commit.short()));
    }
    if args.dry_run {
        println!("{}", msg::revise::DRY_RUN);
        return Ok(true);
    }
    commit(repo, &targets, &counts, other)
}

/// An integration: a merge with a `Prov-Session` trailer.
fn is_integration(commit: &CommitInfo) -> bool {
    commit.parents.len() == 2 && trailer::has(&commit.trailers, trailer::PROV_SESSION)
}

/// The targets of the revision: each `--of`, or the first integration on
/// the first-parent line from `HEAD`.
fn target_commits(repo: &Repo, of: &[String]) -> Result<Vec<CommitInfo>> {
    let mut out: Vec<CommitInfo> = Vec::new();
    for rev in of {
        let sha = repo.rev_parse(rev)?.with_context(|| msg::revise::no_commit(rev))?;
        if out.iter().any(|c| c.sha == sha) {
            continue;
        }
        let commit = repo.commit(&sha)?;
        if !is_integration(&commit) {
            bail!(msg::revise::not_integration(rev));
        }
        if !repo.is_ancestor(&sha, "HEAD")? {
            bail!(msg::revise::not_ancestor(rev));
        }
        out.push(commit);
    }
    if out.is_empty() {
        // `--grep` only narrows the walk; the trailers decide.
        let candidates = repo.commits(["--first-parent", "--merges", "-E", "--grep=^Prov-Session: ", "HEAD"])?;
        out.push(candidates.into_iter().find(is_integration).context(msg::revise::NO_INTEGRATION)?);
    }
    Ok(out)
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

/// The ID of the empty blob, for SHA-1 and SHA-256 repositories.
const EMPTY_BLOBS: [&str; 2] = [
    "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391",
    "473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813",
];

/// The old path of each changed file, in the order of `git diff`. `None` for
/// a file with no old lines to blame: an added file, an empty file or a
/// submodule.
fn old_paths(repo: &Repo) -> Result<Vec<Option<String>>> {
    let raw = repo.git(["diff", "--cached", "-M", "--raw", "--no-abbrev", "-z", "HEAD"])?;
    let mut fields = raw.split('\0');
    let mut out = Vec::new();
    while let Some(meta) = fields.next() {
        if meta.is_empty() {
            continue;
        }
        // `:<old mode> <new mode> <old id> <new id> <status>`, then the
        // path, or two paths for a rename or a copy.
        let parts: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
        let [old_mode, _, old_id, _, status] = parts.as_slice() else { bail!(msg::revise::UNEXPECTED_DIFF) };
        let old = fields.next().context(msg::revise::UNEXPECTED_DIFF)?;
        if status.starts_with('R') || status.starts_with('C') {
            fields.next().context(msg::revise::UNEXPECTED_DIFF)?;
        }
        let blameable = *old_mode != "000000" && *old_mode != "160000" && !EMPTY_BLOBS.contains(old_id);
        out.push(blameable.then(|| old.to_string()));
    }
    Ok(out)
}

/// A hunk of a `-U0` diff.
#[derive(Debug, PartialEq, Eq)]
enum Hunk {
    /// The old lines `start..=end` change or go.
    Changed(u32, u32),
    /// `count` lines go in after old line `after` (0: at the start).
    Inserted { after: u32, count: u32 },
}

/// `@@ -<start>[,<count>] +<start>[,<count>] @@`. With an old count of 0,
/// the old start is the line after which the new lines go.
fn hunk(line: &str) -> Option<Hunk> {
    let rest = line.strip_prefix("@@ -")?;
    let mut words = rest.split(' ');
    let range = |word: &str| -> Option<(u32, u32)> {
        match word.split_once(',') {
            Some((s, c)) => Some((s.parse().ok()?, c.parse().ok()?)),
            None => Some((word.parse().ok()?, 1)),
        }
    };
    let (old_start, old_count) = range(words.next()?)?;
    let (_, new_count) = range(words.next()?.strip_prefix('+')?)?;
    if old_count > 0 {
        Some(Hunk::Changed(old_start, old_start + old_count - 1))
    } else {
        Some(Hunk::Inserted { after: old_start, count: new_count })
    }
}

/// R-REV-1: the hunks of the staged diff, by old path. A new file has no
/// old path, and none of its lines count.
fn staged_diff(repo: &Repo) -> Result<Diff> {
    let paths = old_paths(repo)?;
    let patch = repo.git(["diff", "--cached", "-M", "-U0", "--no-color", "--no-ext-diff", "HEAD"])?;
    let mut diff = Diff::new();
    let mut file: Option<usize> = None;
    for line in patch.lines() {
        if line.starts_with("diff --git ") {
            file = Some(file.map_or(0, |i| i + 1));
            continue;
        }
        let (Some(i), Some(h)) = (file, hunk(line)) else { continue };
        if let Some(old) = paths.get(i).context(msg::revise::UNEXPECTED_DIFF)? {
            let entry = diff.entry(old.clone()).or_default();
            match h {
                Hunk::Changed(start, end) => entry.changed.push((start, end)),
                Hunk::Inserted { after, count } => entry.inserted.push((after, count)),
            }
        }
    }
    Ok(diff)
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

/// One `git blame` call for each file, for the changed lines and the
/// neighbours of the inserted lines, and one `git log` call for the trailers
/// of the blamed commits. Returns the counts of each target, and the number
/// of changed lines that are merged lines of no target.
fn classify(repo: &Repo, diff: &Diff, targets: &[Target]) -> Result<(Vec<Counts>, usize)> {
    let mut blamed: BTreeMap<&str, BTreeMap<u32, String>> = BTreeMap::new();
    for (path, hunks) in diff {
        let mut ranges: Vec<(u32, u32)> = hunks.changed.clone();
        // The neighbours: the line before and the line after. Line 0 does
        // not exist; git clamps a range past the end of the file.
        ranges.extend(hunks.inserted.iter().map(|&(after, _)| (after.max(1), after + 1)));
        let mut args: Vec<String> = vec!["blame".into(), "--porcelain".into(), "HEAD".into()];
        for (start, end) in ranges {
            args.push("-L".into());
            args.push(format!("{start},{end}"));
        }
        args.push("--".into());
        args.push(path.clone());
        let lines = blamed_lines(&repo.git(&args)?).into_iter().map(|(line, c)| (line, c.to_string())).collect();
        blamed.insert(path, lines);
    }
    let unknown: HashSet<&str> = blamed
        .values()
        .flat_map(|lines| lines.values())
        .map(String::as_str)
        .filter(|c| !targets.iter().any(|t| t.merged.contains(*c)))
        .collect();
    let revises = revised_targets(repo, &unknown)?;
    let is_merged = |commit: &str, t: &Target| {
        t.merged.contains(commit) || revises.get(commit).is_some_and(|r| r.contains(&t.commit.sha))
    };

    let mut counts = vec![Counts::default(); targets.len()];
    let mut other = 0;
    for (path, hunks) in diff {
        let lines = &blamed[path.as_str()];
        for &(start, end) in &hunks.changed {
            for line in start..=end {
                let Some(commit) = lines.get(&line) else { continue };
                let mut any = false;
                for (t, c) in targets.iter().zip(counts.iter_mut()) {
                    if is_merged(commit, t) {
                        c.changed += 1;
                        any = true;
                    }
                }
                if !any {
                    other += 1;
                }
            }
        }
        for &(after, count) in &hunks.inserted {
            let neighbours = [after, after + 1];
            for (t, c) in targets.iter().zip(counts.iter_mut()) {
                if neighbours.iter().any(|n| lines.get(n).is_some_and(|commit| is_merged(commit, t))) {
                    c.inserted += count as usize;
                }
            }
        }
    }
    Ok((counts, other))
}

/// The `Prov-Revises` targets of each commit, by one `git log --no-walk`.
/// Every blamed commit is an ancestor of `HEAD`.
fn revised_targets(repo: &Repo, commits: &HashSet<&str>) -> Result<HashMap<String, Vec<String>>> {
    if commits.is_empty() {
        return Ok(HashMap::new());
    }
    let mut args: Vec<String> = vec![
        "log".into(),
        "--no-walk=unsorted".into(),
        format!("--format=%H %(trailers:key={},valueonly,separator=%x20)", trailer::PROV_REVISES),
    ];
    args.extend(commits.iter().map(|c| (*c).to_string()));
    args.push("--".into());
    let text = repo.git(&args)?;
    Ok(text
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let commit = words.next()?.to_string();
            let targets: Vec<String> = words.map(String::from).collect();
            (!targets.is_empty()).then_some((commit, targets))
        })
        .collect())
}

/// The commit of each line in `git blame --porcelain` output, by line
/// number in the blamed revision. Each line has one header: `<commit> <old
/// line> <new line>[ <count>]`. Content lines start with a tab, and no
/// metadata line starts with a hex word.
fn blamed_lines(porcelain: &str) -> BTreeMap<u32, &str> {
    porcelain
        .lines()
        .filter_map(|line| {
            let mut words = line.split(' ');
            let commit = words.next()?;
            if commit.len() < 40 || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            words.next()?.parse::<u32>().ok()?;
            let final_line = words.next()?.parse::<u32>().ok()?;
            Some((final_line, commit))
        })
        .collect()
}

/// Starts `git commit` with a template: an empty subject line, the trailer
/// and comment lines. git refuses the commit if the message is not changed.
fn commit(repo: &Repo, targets: &[Target], counts: &[Counts], other: usize) -> Result<bool> {
    let comment = match repo.config_value("core.commentChar")? {
        None => Some("#".to_string()),
        // With `auto`, git picks a character that no line of the template
        // starts with, so comment lines would stay in the message.
        Some(c) if c == "auto" => None,
        Some(c) => Some(c),
    };
    let mut text = String::from("\n\n");
    for t in targets {
        text.push_str(trailer::PROV_REVISES);
        text.push_str(": ");
        text.push_str(&t.commit.sha);
        text.push('\n');
    }
    if let Some(c) = comment {
        text.push('\n');
        let rows: Vec<(String, String, usize, usize)> = targets
            .iter()
            .zip(counts)
            .map(|(t, n)| (t.commit.short().to_string(), t.commit.subject.clone(), n.changed, n.inserted))
            .collect();
        for line in msg::revise::template_comment(&rows, other).lines() {
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
    fn hunks() {
        assert_eq!(hunk("@@ -3 +3 @@"), Some(Hunk::Changed(3, 3)));
        assert_eq!(hunk("@@ -3,2 +3 @@ fn x"), Some(Hunk::Changed(3, 4)));
        assert_eq!(hunk("@@ -3,0 +4,2 @@"), Some(Hunk::Inserted { after: 3, count: 2 }));
        assert_eq!(hunk("@@ -0,0 +1 @@"), Some(Hunk::Inserted { after: 0, count: 1 }));
        assert_eq!(hunk("@@ -1,4 +0,0 @@"), Some(Hunk::Changed(1, 4)), "deleted file");
        assert_eq!(hunk("+@@ -1 +1 @@"), None);
    }

    #[test]
    fn porcelain_headers() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let text = format!(
            "{a} 3 3 2\nauthor X\nsummary 1234567890123456789012345678901234567890 9 9\n\tline\n{a} 4 4\n\tline\n{b} 7 8 1\nprevious {a} f\n\t{b} 1 1\n"
        );
        let lines: Vec<(u32, &str)> = blamed_lines(&text).into_iter().collect();
        assert_eq!(lines, [(3, a.as_str()), (4, a.as_str()), (8, b.as_str())]);
    }
}
