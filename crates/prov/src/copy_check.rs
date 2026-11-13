//! What happens to a copy of `prov cp` after the files are staged: the copy
//! guard in `pre-commit`, the `Copied-From` trailers, and the deferred check
//! of the note against the plan.

use std::collections::BTreeMap;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use prov_core::plan::{PlanFile, Who};
use prov_core::trailer::{self, Trailer};
use prov_core::{now, CopyCheck, CopyFile, CopyRecord};
use prov_gitai::{Author, AWAIT_TIMEOUT_SECS};

use crate::context::{say, Ctx};

/// The copies of this worktree with a file that no commit holds yet.
fn pending(ctx: &Ctx) -> Result<Vec<CopyRecord>> {
    Ok(ctx
        .store
        .copies()?
        .into_iter()
        .filter(|r| r.worktree == ctx.repo.toplevel && r.files.iter().any(|f| f.commit.is_none()))
        .collect())
}

/// The mode and the blob of each path in the index, for the paths that the
/// commit in progress changes.
fn staged(ctx: &Ctx) -> Result<BTreeMap<String, (String, String)>> {
    let repo = &ctx.repo;
    let changed = repo.git(["diff", "--cached", "--name-only", "--no-renames", "-z"])?;
    let changed: Vec<&str> = changed.split('\0').filter(|p| !p.is_empty()).collect();
    let mut out = BTreeMap::new();
    if changed.is_empty() {
        return Ok(out);
    }
    let text = repo.git(["ls-files", "-s", "-z"])?;
    for record in text.split('\0') {
        // `<mode> <blob> <stage>\t<path>`
        let Some((meta, path)) = record.split_once('\t') else { continue };
        if !changed.contains(&path) {
            continue;
        }
        let mut parts = meta.split(' ');
        if let (Some(mode), Some(blob)) = (parts.next(), parts.next()) {
            out.insert(path.to_string(), (mode.to_string(), blob.to_string()));
        }
    }
    Ok(out)
}

fn is_copied_bytes(file: &CopyFile, entry: &(String, String)) -> bool {
    entry.0 == file.mode && entry.1 == file.blob
}

/// The copy guard of `pre-commit`: a copied file in the commit must be equal
/// to the copied bytes. Returns false to refuse the commit.
pub fn guard(ctx: &Ctx) -> Result<bool> {
    let records = pending(ctx)?;
    if records.is_empty() {
        return Ok(true);
    }
    let staged = staged(ctx)?;
    let mut ok = true;
    for file in records.iter().flat_map(|r| &r.files).filter(|f| f.commit.is_none()) {
        let Some(entry) = staged.get(&file.dest) else { continue };
        if !is_copied_bytes(file, entry) {
            say(format!(
                "refused: {} is a file of `prov cp`, and it is different from the copied bytes. Commit the copy \
                 unchanged; change the file in a later commit.\nTo give up the copy: git rm -f --cached -- {} (and \
                 delete the file), then copy it again if necessary.",
                file.dest, file.dest
            ));
            ok = false;
        }
    }
    Ok(ok)
}

/// `prepare-commit-msg`: one `Copied-From` trailer for each copied file of the commit.
pub fn add_trailers(ctx: &Ctx, message_file: &std::path::Path) -> Result<()> {
    let records = pending(ctx)?;
    if records.is_empty() {
        return Ok(());
    }
    let staged = staged(ctx)?;
    let existing = ctx.repo.message_trailers(message_file)?;
    let mut add = Vec::new();
    for record in &records {
        for file in record.files.iter().filter(|f| f.commit.is_none()) {
            // `--no-verify` skips the guard: a file that changed gets no trailer.
            if !staged.get(&file.dest).is_some_and(|entry| is_copied_bytes(file, entry)) {
                continue;
            }
            let value = trailer::copied_from_value(&record.source, &record.source_commit, &file.path, &file.dest);
            if !existing.iter().any(|t| t.key == trailer::COPIED_FROM && t.value == value) {
                add.push(Trailer::new(trailer::COPIED_FROM, value)?);
            }
        }
    }
    ctx.repo.add_trailers(message_file, &add)
}

/// `post-commit`: records the commit of each copied file that it holds, and
/// starts the deferred check.
pub fn committed(ctx: &Ctx, head: &str) -> Result<()> {
    let records = pending(ctx)?;
    if records.is_empty() {
        return Ok(());
    }
    let changed = ctx.repo.git(["diff-tree", "--no-commit-id", "--name-only", "--no-renames", "-r", "--root", "-z", head])?;
    let changed: Vec<&str> = changed.split('\0').filter(|p| !p.is_empty()).collect();
    let lock = ctx.store.lock()?;
    let mut started = Vec::new();
    for mut record in records {
        let mut touched = false;
        for file in record.files.iter_mut().filter(|f| f.commit.is_none()) {
            if changed.contains(&file.dest.as_str()) {
                file.commit = Some(head.to_string());
                file.committed_at = Some(now());
                touched = true;
            }
        }
        if touched {
            lock.put_copy(&record)?;
            started.push(record.id);
        }
    }
    drop(lock);
    for id in started {
        // The check waits for the note. git gives a hook no time for that.
        if let Err(e) = spawn_detached(ctx, &id) {
            say(format!("warning: the copy check did not start: {e:#}. Run it: prov copy-check {id}"));
        }
    }
    Ok(())
}

/// Starts `prov copy-check <id>` in its own process session.
fn spawn_detached(ctx: &Ctx, id: &str) -> Result<u32> {
    let exe = std::env::current_exe().context("cannot find the prov binary")?;
    let mut cmd = Command::new(exe);
    cmd.args(["copy-check", id]).current_dir(&ctx.repo.toplevel).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // The hook environment names an index file that git removes.
    for var in ["GIT_INDEX_FILE", "GIT_DIR", "GIT_WORK_TREE", "GIT_PREFIX"] {
        cmd.env_remove(var);
    }
    // SAFETY: setsid is async-signal-safe and touches no memory of the parent.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    Ok(cmd.spawn().context("cannot start the copy check")?.id())
}

fn same_author(plan: &Who, actual: &Author) -> bool {
    match (plan, actual) {
        (Who::Human, Author::Human) => true,
        (Who::Ai { agent, model, session }, Author::Ai(s)) => *agent == s.tool && *model == s.model && *session == s.id,
        _ => false,
    }
}

fn describe(actual: &Author) -> String {
    match actual {
        Author::Human => "human".into(),
        Author::Ai(s) => format!("ai {}/{} ({})", s.tool, s.model, s.id),
        Author::Untracked => "no author".into(),
    }
}

/// Compares the authors of the note with the plan, by content: in each group
/// of identical lines, the set of authors must agree with the plan.
pub fn compare(lines: &[&[u8]], plan: &[&Who], actual: &[Author]) -> (CopyCheck, Option<String>) {
    if lines.len() != plan.len() || actual.len() != plan.len() {
        return (CopyCheck::Failed, Some(format!("the plan has {} lines, and the committed file has {}", plan.len(), lines.len())));
    }
    let differ: Vec<usize> = (0..plan.len()).filter(|&i| !same_author(plan[i], &actual[i])).collect();
    let Some(&first) = differ.first() else { return (CopyCheck::Agrees, None) };
    // Group by content. A line of the note takes an author of the plan away from its group.
    let mut groups: BTreeMap<&[u8], Vec<&Who>> = BTreeMap::new();
    for (line, who) in lines.iter().zip(plan) {
        groups.entry(line).or_default().push(who);
    }
    for (line, author) in lines.iter().zip(actual) {
        // Each line is in a group: the loop above made one for every line.
        let Some(group) = groups.get_mut(line) else { return (CopyCheck::Failed, Some(format!("line {}: no group", first + 1))) };
        match group.iter().position(|who| same_author(who, author)) {
            Some(at) => {
                group.swap_remove(at);
            }
            None => {
                return (
                    CopyCheck::Failed,
                    Some(format!(
                        "line {}: the plan has `{}`, and the note has `{}` ({} line(s) differ)",
                        first + 1,
                        plan[first],
                        describe(&actual[first]),
                        differ.len()
                    )),
                );
            }
        }
    }
    (CopyCheck::Moved, Some(format!("the authors of {} line(s) moved between identical lines", differ.len())))
}

/// True if `refs/notes/ai` has a note for the commit. This is a read of git;
/// the content of the note comes only from git-ai.
fn has_note(ctx: &Ctx, commit: &str) -> bool {
    ctx.repo.git_ok(["notes", "--ref=ai", "list", commit]).unwrap_or(false)
}

/// The deferred check of one copy, for the detached process.
pub fn run(id: &str) -> Result<()> {
    let here = Ctx::here()?;
    let path = here.store.copy_path(id)?;
    let read = || -> Result<Option<CopyRecord>> {
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes).with_context(|| format!("{} does not parse", path.display()))?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    };
    let Some(record) = read()? else { return Ok(()) };
    let ctx = Ctx::discover(&record.worktree)?.with_context(|| format!("{} is not a worktree", record.worktree.display()))?;

    // Wait for the note of each commit, for a maximum of 60 seconds. The
    // service learns of a commit only when `git commit` ends, after `post-commit`.
    let commits: Vec<String> = {
        let mut list: Vec<String> =
            record.files.iter().filter(|f| f.check.is_none()).filter_map(|f| f.commit.clone()).collect();
        list.sort();
        list.dedup();
        list
    };
    let begin = Instant::now();
    loop {
        let _ = ctx.gitai.await_notes(&ctx.repo.toplevel);
        if commits.iter().all(|c| has_note(&ctx, c)) || begin.elapsed() >= Duration::from_secs(u64::from(AWAIT_TIMEOUT_SECS)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    let mut results: Vec<(String, CopyCheck, Option<String>)> = Vec::new();
    for file in record.files.iter().filter(|f| f.check.is_none()) {
        let Some(commit) = &file.commit else { continue };
        let result = check_file(&ctx, file, commit).unwrap_or_else(|e| (CopyCheck::Failed, Some(format!("{e:#}"))));
        results.push((file.dest.clone(), result.0, result.1));
    }

    // Read the record again under the lock: another commit can have changed it.
    let lock = ctx.store.lock()?;
    let Some(mut record) = read()? else { return Ok(()) };
    for (dest, check, detail) in &results {
        if let Some(file) = record.files.iter_mut().find(|f| f.dest == *dest && f.check.is_none() && f.commit.is_some()) {
            file.check = Some(*check);
            file.detail.clone_from(detail);
        }
    }
    // A later copy of the same path that agrees clears an earlier failure.
    let agreed: Vec<&String> = results.iter().filter(|(_, check, _)| *check == CopyCheck::Agrees).map(|(dest, _, _)| dest).collect();
    for mut old in lock.copies()? {
        if old.id == record.id || old.worktree != record.worktree {
            continue;
        }
        let before = old.files.len();
        old.files.retain(|f| !(f.check == Some(CopyCheck::Failed) && agreed.contains(&&f.dest)));
        if old.files.len() == before {
            continue;
        }
        if old.files.is_empty() || old.settle() {
            lock.delete_copy(&old.id)?;
        } else {
            lock.put_copy(&old)?;
        }
    }
    if record.settle() {
        lock.delete_copy(&record.id)?;
    } else {
        lock.put_copy(&record)?;
    }
    Ok(())
}

fn check_file(ctx: &Ctx, file: &CopyFile, commit: &str) -> Result<(CopyCheck, Option<String>)> {
    let repo = &ctx.repo;
    let blob = repo.git_opt(["rev-parse", "-q", "--verify", &format!("{commit}:{}", file.dest)])?;
    if blob.as_deref() != Some(file.blob.as_str()) {
        return Ok((CopyCheck::Failed, Some(format!("the file in the commit {commit} is different from the copied bytes"))));
    }
    // A binary file and a symbolic link have no line authorship.
    if !file.text {
        return Ok((CopyCheck::Agrees, None));
    }
    if !has_note(ctx, commit) {
        return Ok((CopyCheck::Failed, Some(format!("the commit {commit} has no git-ai note after {AWAIT_TIMEOUT_SECS} seconds"))));
    }
    let bytes = repo.git_bytes(["cat-file", "blob", &file.blob])?;
    let lines: Vec<&[u8]> = bytes.split_inclusive(|b| *b == b'\n').collect();
    let plan_file = PlanFile { path: file.path.clone(), dest: file.dest.clone(), binary: false, symlink: false, ranges: file.ranges.clone() };
    plan_file.check_cover(u32::try_from(lines.len()).context("the file has too many lines")?)?;
    let plan = plan_file.line_authors();
    let mut actual = vec![Author::Untracked; lines.len()];
    let added: Vec<_> = ctx.gitai.diff(&repo.toplevel, commit)?.added.into_iter().filter(|r| r.path == file.dest).collect();
    if added.is_empty() && !lines.is_empty() {
        // git-ai ignores some paths, for example `**/vendor/**` and lock
        // files. It then drops the checkpoints and lists no line of the file.
        return Ok((
            CopyCheck::Ignored,
            Some(format!(
                "git-ai lists no line of {} in the commit {commit}. git-ai ignores some paths (for example \
                 `**/vendor/**` and lock files) and records no authorship for them",
                file.dest
            )),
        ));
    }
    for range in added {
        for line in range.start..=range.end {
            if let Some(slot) = actual.get_mut(line as usize - 1) {
                *slot = range.author.clone();
            }
        }
    }
    Ok(compare(&lines, &plan, &actual))
}

#[cfg(test)]
mod tests {
    use super::*;
    use prov_gitai::AiSession;

    fn ai(id: &str) -> Who {
        Who::Ai { agent: "claude".into(), model: "m1".into(), session: id.into() }
    }

    fn note_ai(id: &str, model: &str) -> Author {
        Author::Ai(AiSession { tool: "claude".into(), id: id.into(), model: model.into() })
    }

    #[test]
    fn compares_by_content() {
        let lines: Vec<&[u8]> = vec![b"x\n", b"same\n", b"same\n", b"y\n"];
        let (h, s1) = (Who::Human, ai("s1"));
        let plan = vec![&h, &h, &s1, &s1];
        // The exact plan.
        let exact = [Author::Human, Author::Human, note_ai("s1", "m1"), note_ai("s1", "m1")];
        assert_eq!(compare(&lines, &plan, &exact).0, CopyCheck::Agrees);
        // The authors of the two identical lines changed places.
        let moved = [Author::Human, note_ai("s1", "m1"), Author::Human, note_ai("s1", "m1")];
        let (check, detail) = compare(&lines, &plan, &moved);
        assert_eq!(check, CopyCheck::Moved);
        assert!(detail.unwrap().contains("2 line(s) moved"));
        // Both identical lines got the session: the counts do not agree.
        let both = [Author::Human, note_ai("s1", "m1"), note_ai("s1", "m1"), note_ai("s1", "m1")];
        assert_eq!(compare(&lines, &plan, &both).0, CopyCheck::Failed);
        // Another model.
        let model = [Author::Human, Author::Human, note_ai("s1", "m2"), note_ai("s1", "m2")];
        let (check, detail) = compare(&lines, &plan, &model);
        assert_eq!(check, CopyCheck::Failed);
        assert!(detail.unwrap().contains("line 3"));
        // No note: no line has an author.
        let none = vec![Author::Untracked; 4];
        assert_eq!(compare(&lines, &plan, &none).0, CopyCheck::Failed);
        assert_eq!(compare(&lines[..3], &plan, &exact).0, CopyCheck::Failed);
    }
}
