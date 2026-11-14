//! `prov discard`: SPEC.md, "Discarding" (R-DISC-1).

// A panic in a test is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};

use common::{branch, Env};
use serde_json::Value;

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

/// A worktree with one session that committed and ended. Returns the
/// worktree and the archive file.
fn archived_branch(env: &Env, repo: &Path, name: &str, session: &str) -> (PathBuf, PathBuf) {
    let wt = env.worktree(repo, name);
    env.start_session(&wt, session).success();
    env.agent_edit(&wt, "a.py", "a = 1\n", session);
    env.agent_commit(&wt, session, "Add a").success();
    let record = env.end_session(&wt, session);
    (wt, PathBuf::from(record["archive_file"].as_str().unwrap()))
}

fn branch_exists(env: &Env, repo: &Path, name: &str) -> bool {
    env.try_git(repo, ["rev-parse", "-q", "--verify", &format!("refs/heads/{}", branch(name))]).ok
}

#[test]
fn discard_lists_then_deletes_everything_of_the_branch() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let (wt, archive) = archived_branch(&env, &repo, "failed", "sess-a");
    // A second session that made no commit, and a file that is not committed.
    env.start_session(&wt, "sess-idle").success();
    let idle = env.end_session(&wt, "sess-idle");
    let idle_archive = PathBuf::from(idle["archive_file"].as_str().unwrap());
    env.plain_write(&wt, "scratch.txt", "x\n");
    let transcript = env.transcript_path("sess-a");

    // --dry-run and no --yes both list and delete nothing.
    let dry = json(&env.prov(&repo, ["discard", &branch("failed"), "--dry-run", "--json"]).success().stdout);
    assert_eq!(dry["deleted"], false);
    assert_eq!(dry["own_commits"], 1);
    assert_eq!(dry["changed_files"], 1);
    assert_eq!(dry["sessions"].as_array().unwrap().len(), 2, "{dry}");
    assert_eq!(dry["archives"].as_array().unwrap().len(), 2, "{dry}");
    let ask = env.prov(&repo, ["discard", "sess-a"]).success();
    assert!(ask.stdout.contains("Nothing was deleted") && ask.stdout.contains("--yes"), "{}", ask.stdout);
    assert!(wt.is_dir() && archive.is_file() && env.record(&repo, "sess-a").is_some());

    // By session ID, with --yes.
    let done = env.prov(&repo, ["discard", "sess-a", "--yes"]).success();
    assert!(done.stdout.contains(&format!("Discarded the branch {}", branch("failed"))), "{}", done.stdout);
    assert!(!wt.exists());
    assert!(!branch_exists(&env, &repo, "failed"));
    assert!(env.record(&repo, "sess-a").is_none() && env.record(&repo, "sess-idle").is_none());
    assert!(!archive.exists() && !idle_archive.exists());
    // The agent's own transcript stays.
    assert!(transcript.is_file());
}

#[test]
fn discard_refuses_what_another_ref_or_a_live_session_needs() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");

    // An agent may not run it.
    let (wt, archive) = archived_branch(&env, &repo, "kept", "sess-k");
    let agent = env.run(&repo, "prov", ["discard", &branch("kept"), "--yes"], &[("CLAUDECODE", "1")], None).failure();
    assert!(agent.stderr.contains("only a human"), "{}", agent.stderr);
    // Not from the branch's own worktree.
    let here = env.prov(&wt, ["discard", &branch("kept"), "--yes"]).failure();
    assert!(here.stderr.contains("agent worktree"), "{}", here.stderr);

    // An Open session.
    env.start_session(&wt, "sess-open").success();
    let open = env.prov(&repo, ["discard", &branch("kept"), "--yes"]).failure();
    assert!(open.stderr.contains("sess-open (open)"), "{}", open.stderr);
    env.end_session(&wt, "sess-open");

    // A merge of the branch in progress (R-DISC-1).
    env.git(&repo, ["merge", "--no-commit", &branch("kept")]);
    let merging = env.prov(&repo, ["discard", &branch("kept"), "--yes"]).failure();
    assert!(merging.stderr.contains("merge of") && merging.stderr.contains("in progress"), "{}", merging.stderr);
    env.git(&repo, ["merge", "--abort"]);

    // Another agent branch built on this one names its session.
    env.worktree_from(&repo, "child", &branch("kept"));
    let child = env.prov(&repo, ["discard", &branch("kept"), "--yes"]).failure();
    assert!(child.stderr.contains("names session claude:sess-k"), "{}", child.stderr);
    env.git(&repo, ["worktree", "remove", "--force", &env.repos().join("project.worktrees/child").to_string_lossy()]);
    env.git(&repo, ["branch", "-D", &branch("child")]);

    // After the integration, a session that main names and that still has a
    // record (here: resumed, R-INT-2 keeps it) is refused.
    let idle_archive = PathBuf::from(env.record(&repo, "sess-open").unwrap()["archive_file"].as_str().unwrap().to_string());
    env.try_git(&repo, ["merge", &branch("kept")]).success();
    assert!(env.record(&repo, "sess-k").is_none());
    env.start_session(&wt, "sess-k").success();
    env.end_session(&wt, "sess-k");
    let merged = env.prov(&repo, ["discard", &branch("kept"), "--yes"]).failure();
    assert!(merged.stderr.contains("on another ref names session claude:sess-k"), "{}", merged.stderr);
    assert!(merged.stderr.contains("prov finish"), "{}", merged.stderr);
    assert!(archive.is_file() && wt.is_dir() && branch_exists(&env, &repo, "kept"));

    // With that record dropped, the branch has no own commits and no session
    // that another ref needs: discard removes the rest. The archive that the
    // merge names was never a part of it.
    env.prov(&repo, ["session", "drop", "sess-k"]).success();
    let rest = json(&env.prov(&repo, ["discard", &branch("kept"), "--yes", "--json"]).success().stdout);
    assert_eq!(rest["own_commits"], 0);
    assert_eq!(rest["sessions"].as_array().unwrap().len(), 1, "{rest}");
    assert!(!idle_archive.exists() && !wt.exists() && !branch_exists(&env, &repo, "kept"));
    assert!(archive.is_file());
}

/// A killed archive step leaves a temporary file with part of the
/// transcript. `prov discard` deletes it with the archives.
#[test]
fn discard_deletes_the_temporary_file_of_a_killed_archive_step() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let (_, archive) = archived_branch(&env, &repo, "leftover", "sess-left");
    let name = archive.file_name().unwrap().to_string_lossy().replace(".tar.gz", ".tar.tmp999999");
    let temp = archive.with_file_name(name);
    std::fs::write(&temp, b"part of a transcript").unwrap();
    let out = json(&env.prov(&repo, ["discard", &branch("leftover"), "--yes", "--json"]).success().stdout);
    assert!(out["archives"].as_array().unwrap().iter().any(|a| a.as_str() == temp.to_str()), "{out}");
    assert!(!temp.exists() && !archive.exists());
}

#[test]
fn discard_finishes_an_interrupted_discard() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let (wt, archive) = archived_branch(&env, &repo, "half", "sess-h");
    // As if a discard stopped after step 2: records and archives are gone.
    env.prov(&repo, ["session", "drop", "sess-h"]).success();
    std::fs::remove_file(&archive).unwrap();

    let dry = json(&env.prov(&repo, ["discard", &branch("half"), "--dry-run", "--json"]).success().stdout);
    // The commit still names the session; it has no record and no archive.
    assert_eq!(dry["sessions"][0]["id"], "sess-h");
    assert_eq!(dry["sessions"][0]["state"], Value::Null);
    assert_eq!(dry["archives"].as_array().unwrap().len(), 0);
    env.prov(&repo, ["discard", &branch("half"), "--yes"]).success();
    assert!(!wt.exists() && !branch_exists(&env, &repo, "half"));
}

/// `prov discard` and a real `git merge` of the same branch, started at the
/// same time, many times. Whatever order the processes take, every
/// transcript hash that a merge commit records has its archive
/// (`ProvDiscard`: `EvidenceKept`). Either the merge records the session and
/// its archive stays, or the discard deletes it and no merge records it.
#[test]
fn discard_racing_a_merge_keeps_the_evidence() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let (mut merged, mut discarded, mut neither) = (0, 0, 0);
    for round in 0..12u32 {
        let name = format!("race-{round}");
        let session = format!("sess-race-{round}");
        let wt = env.worktree(&repo, &name);
        env.start_session(&wt, &session).success();
        env.agent_edit(&wt, &format!("{name}.py"), "a = 1\n", &session);
        env.agent_commit(&wt, &session, "Add a file").success();
        let archive = PathBuf::from(env.end_session(&wt, &session)["archive_file"].as_str().unwrap());
        let head = env.git(&repo, ["rev-parse", "HEAD"]);
        // Half the rounds start the merge late, half the discard, by 0 to 250 ms.
        let late = std::time::Duration::from_millis(50 * u64::from(round / 2 % 6));
        let (merge_delay, discard_delay) = if round % 2 == 0 { (late, std::time::Duration::ZERO) } else { (std::time::Duration::ZERO, late) };
        let (merge, discard) = std::thread::scope(|s| {
            let m = s.spawn(|| {
                std::thread::sleep(merge_delay);
                env.try_git(&repo, ["merge", "-m", "Merge", &branch(&name)])
            });
            let d = s.spawn(|| {
                std::thread::sleep(discard_delay);
                env.prov(&repo, ["discard", &branch(&name), "--yes"])
            });
            (m.join().unwrap(), d.join().unwrap())
        });
        if !merge.ok {
            // A gate that refused leaves the merge in progress.
            let _ = env.try_git(&repo, ["merge", "--abort"]);
        }
        let new_head = env.git(&repo, ["rev-parse", "HEAD"]);
        let hashes: Vec<String> = env
            .trailers(&repo, "HEAD")
            .iter()
            .filter_map(|t| t.strip_prefix("Prov-Transcript-SHA256: ").map(String::from))
            .collect();
        if new_head != head && !hashes.is_empty() {
            // EvidenceKept: the archive of the recorded hash exists, with that hash.
            assert!(archive.is_file(), "round {round}: merge recorded {hashes:?}, archive deleted\n{}\n{}", merge.stderr, discard.stderr);
            assert_eq!(common::sha256_of_gunzipped(&archive), hashes[0], "round {round}");
            // A discard after the merge finds no own commits and no records
            // (post-commit deleted them, R-INT-2): it may remove the worktree
            // and the branch, and keeps the archive.
            merged += 1;
        } else if discard.ok {
            assert!(!archive.exists() && !branch_exists(&env, &repo, &name), "round {round}");
            discarded += 1;
        } else {
            neither += 1;
        }
        assert_eq!(env.git(&repo, ["status", "--porcelain"]), "", "round {round}");
    }
    eprintln!("discard racing a merge: {merged} merged, {discarded} discarded, {neither} neither");
    assert_eq!(merged + discarded + neither, 12);
}

/// `prov discard` and an agent event of the same session (a resume), at the
/// same time. Either the discard refuses because the session is Open, or it
/// deleted the session first. No Open record is left for a branch that is
/// gone, and no hook fails.
#[test]
fn discard_racing_a_resume() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let (mut refused, mut deleted, mut orphans) = (0, 0, 0);
    for round in 0..8 {
        let name = format!("resume-{round}");
        let session = format!("sess-resume-{round}");
        let wt = env.worktree(&repo, &name);
        env.start_session(&wt, &session).success();
        env.agent_edit(&wt, &format!("{name}.py"), "a = 1\n", &session);
        env.agent_commit(&wt, &session, "Add a file").success();
        env.end_session(&wt, &session);
        let delay = std::time::Duration::from_millis(10 * (round % 4));
        let (event, discard) = std::thread::scope(|s| {
            let e = s.spawn(|| {
                std::thread::sleep(delay);
                env.hook(&wt, "UserPromptSubmit", &session)
            });
            let d = s.spawn(|| env.prov(&repo, ["discard", &branch(&name), "--yes"]));
            (e.join().unwrap(), d.join().unwrap())
        });
        let record = env.record(&repo, &session);
        if discard.ok {
            assert!(!branch_exists(&env, &repo, &name) && !wt.exists(), "round {round}");
            // The worktree goes under the lock before the records: an event
            // after the discard finds no worktree and makes no record.
            if record.as_ref().is_some_and(|r| r["state"] == "open") {
                orphans += 1;
            }
            deleted += 1;
        } else {
            assert!(discard.stderr.contains("open") || discard.stderr.contains("changed"), "round {round}: {}", discard.stderr);
            assert!(branch_exists(&env, &repo, &name), "round {round}");
            refused += 1;
        }
        assert!(event.ok, "round {round}: the hook failed: {}", event.stderr);
    }
    eprintln!("discard racing a resume: {refused} refused, {deleted} deleted, {orphans} new Open record(s) after the discard");
    assert_eq!(orphans, 0, "an Open record for a removed worktree");
}
