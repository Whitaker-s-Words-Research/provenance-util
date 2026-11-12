//! Session states, binding rules and the agent hooks.

mod common;

use std::time::{Duration, Instant};

use common::{branch, Env};

#[test]
fn contended_worktree() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = env.worktree(&repo, "shared");

    env.start_session(&wt, "sess-one").success();
    // prov cannot stop a second session. It records it and tells it to stop.
    let second = env.start_session(&wt, "sess-two").success();
    assert!(second.stdout.contains("STOP") && second.stdout.contains("sess-one"), "{}", second.stdout);
    assert_eq!(env.state(&repo, "sess-one"), "open");
    assert_eq!(env.state(&repo, "sess-two"), "open");

    // No sweep in a contended worktree, and pre-commit refuses every commit.
    env.plain_write(&wt, "a.txt", "one\n");
    env.hook(&wt, "Stop", "sess-one").success();
    let commit = env.agent_commit(&wt, "sess-one", "Add a").failure();
    assert!(commit.stderr.contains("contended"), "{}", commit.stderr);

    // The worktree stays contended until only one session is Open. A Closing
    // session does not occupy the worktree.
    env.end_session(&wt, "sess-two");
    env.agent_commit(&wt, "sess-one", "Add a").success();
    assert_eq!(env.trailers(&wt, "HEAD"), ["Prov-Session: claude:sess-one"]);

    // After a session ends, a new session can bind to the same worktree and branch.
    env.end_session(&wt, "sess-one");
    let third = env.start_session(&wt, "sess-three").success();
    assert_eq!(third.stdout, "");
    env.plain_write(&wt, "b.txt", "two\n");
    env.agent_commit(&wt, "sess-three", "Add b").success();
    assert_eq!(env.trailers(&wt, "HEAD"), ["Prov-Session: claude:sess-three"]);
}

#[test]
fn session_outside_an_agent_worktree() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");

    // In the human worktree: a warning for the agent, and no record.
    let start = env.start_session(&repo, "sess-main").success();
    assert!(start.stdout.contains("not in an agent worktree"), "{}", start.stdout);
    env.hook(&repo, "UserPromptSubmit", "sess-main").success();
    env.hook(&repo, "Stop", "sess-main").success();
    env.hook(&repo, "SessionEnd", "sess-main").success();
    assert!(env.record(&repo, "sess-main").is_none());
    assert!(!repo.join("llm-logs").exists());

    // pre-commit refuses a commit by an agent on the human branch: by the
    // environment variable, and by the author email alone.
    env.plain_write(&repo, "x.txt", "x\n");
    let refused = env.agent_commit(&repo, "sess-main", "Add x").failure();
    assert!(refused.stderr.contains("a commit by an agent"), "{}", refused.stderr);
    let by_email = env.run(
        &repo,
        "git",
        ["commit", "-q", "-m", "Add x"],
        &[("GIT_AUTHOR_NAME", common::AGENT_NAME), ("GIT_AUTHOR_EMAIL", common::AGENT_EMAIL)],
        None,
    );
    assert!(by_email.failure().stderr.contains("the author email is noreply@anthropic.com"));

    // A human can still commit there, and gets no Prov-Session trailer.
    env.git(&repo, ["commit", "-q", "-m", "Add x"]);
    assert!(env.trailers(&repo, "HEAD").is_empty());

    // The same holds in a worktree on a branch that does not match `agent/**`.
    let other = env.home().join("feature");
    env.git(&repo, ["worktree", "add", "-q", "-b", "feature/x", other.to_str().unwrap()]);
    let start = env.start_session(&other, "sess-feature").success();
    assert!(start.stdout.contains("not in an agent worktree"), "{}", start.stdout);
    assert!(env.record(&repo, "sess-feature").is_none());
}

#[test]
fn first_event_records_a_session_and_resume_returns_to_open() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = env.worktree(&repo, "resume");

    // Not every agent has a start event: the first event records the session.
    env.write_transcript("sess-r", &wt);
    env.hook(&wt, "UserPromptSubmit", "sess-r").success();
    assert_eq!(env.state(&repo, "sess-r"), "open");
    assert_eq!(env.record(&repo, "sess-r").unwrap()["model"], "unknown");

    let first = env.end_session(&wt, "sess-r");
    let first_archive = first["archive_file"].as_str().unwrap().to_string();

    // A resumed Claude Code session keeps its session ID. It returns to Open.
    env.hook(&wt, "SessionStart", "sess-r").success();
    let record = env.record(&repo, "sess-r").unwrap();
    assert_eq!(record["state"], "open");
    assert!(record["archive_sha256"].is_null());

    // The transcript grew. Each archive of a session is a separate file, so
    // the hash in an earlier integration stays correct.
    let transcript = env.transcript_path("sess-r");
    let mut text = std::fs::read_to_string(&transcript).unwrap();
    text.push_str("{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"more\"}}\n");
    std::fs::write(&transcript, text).unwrap();
    let second = env.end_session(&wt, "sess-r");
    let second_archive = second["archive_file"].as_str().unwrap().to_string();
    assert_ne!(first_archive, second_archive);
    assert_ne!(first["archive_sha256"], second["archive_sha256"]);
    assert_eq!(std::fs::read_dir(repo.join("llm-logs/claude")).unwrap().count(), 2);
}

#[test]
fn a_session_with_no_transcript_fails_and_blocks_the_integration() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = env.worktree(&repo, "lost");

    // No transcript file exists for this session.
    env.hook(&wt, "SessionStart", "sess-lost").success();
    env.agent_edit(&wt, "a.py", "a = 1\n", "sess-lost");
    env.agent_commit(&wt, "sess-lost", "Add a").success();
    env.hook(&wt, "SessionEnd", "sess-lost").success();
    let record = env.wait_state(&repo, "sess-lost", "failed");
    assert!(record["error"].as_str().unwrap().contains("does not exist"), "{record}");

    // The gate needs the transcript hash of every session.
    let merge = env.try_git(&repo, ["merge", &branch("lost")]).failure();
    assert!(merge.stderr.contains("session claude:sess-lost is failed"), "{}", merge.stderr);
    env.git(&repo, ["merge", "--abort"]);
}

#[test]
fn pre_commit_refuses_in_an_agent_worktree_without_the_service() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = env.worktree(&repo, "noservice");
    env.start_session(&wt, "sess-n").success();
    env.plain_write(&wt, "a.txt", "a\n");
    env.plain_write(&repo, "h.txt", "h\n");

    env.stop_service();
    // A checkpoint would now exit 0 and record nothing.
    let refused = env.agent_commit(&wt, "sess-n", "Add a").failure();
    assert!(refused.stderr.contains("the git-ai service is not available"), "{}", refused.stderr);
    // On the human branch, pre-commit only shows a warning.
    env.git(&repo, ["add", "-A"]);
    let human = env.try_git(&repo, ["commit", "-q", "-m", "Add h"]).success();
    assert!(human.stderr.contains("warning: the git-ai service is not available"), "{}", human.stderr);

    // SessionStart shows a warning and starts the service.
    let start = env.hook(&wt, "SessionStart", "sess-n").success();
    assert!(start.stdout.contains("the git-ai service is not available"), "{}", start.stdout);
    let begin = Instant::now();
    loop {
        let commit = env.agent_commit(&wt, "sess-n", "Add a");
        if commit.ok {
            break;
        }
        assert!(begin.elapsed() < Duration::from_secs(20), "the service did not start: {}", commit.stderr);
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// An agent hook returns in less than 100 ms at the 95th percentile.
#[test]
fn agent_hooks_are_fast() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = env.worktree(&repo, "latency");
    env.start_session(&wt, "sess-fast").success();
    for i in 0..20 {
        env.plain_write(&wt, &format!("gen/file{i}.txt"), "line\n");
    }
    for event in ["SessionStart", "UserPromptSubmit", "Stop"] {
        let mut times: Vec<Duration> = (0..40)
            .map(|_| {
                let begin = Instant::now();
                env.hook(&wt, event, "sess-fast").success();
                begin.elapsed()
            })
            .collect();
        times.sort();
        let p95 = times[times.len() * 95 / 100 - 1];
        eprintln!("{event}: median {:?}, p95 {:?}", times[times.len() / 2], p95);
        assert!(p95 < Duration::from_millis(100), "{event} takes {p95:?} at the 95th percentile");
    }
    // SessionEnd only marks the session and starts a detached process.
    let begin = Instant::now();
    env.hook(&wt, "SessionEnd", "sess-fast").success();
    let end = begin.elapsed();
    eprintln!("SessionEnd: {end:?}");
    assert!(end < Duration::from_millis(100), "SessionEnd takes {end:?}");
    env.wait_state(&repo, "sess-fast", "archived");
}

/// A session whose agent process is gone is stale. It does not make the
/// worktree contended, but it must be closed before the branch integrates.
#[test]
fn a_stale_session_does_not_occupy_the_worktree() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = env.worktree(&repo, "stale");
    // No process has this ID.
    env.write_transcript("sess-gone", &wt);
    env.hook_from(&wt, "SessionStart", "sess-gone", i32::MAX as u32).success();
    assert_eq!(env.state(&repo, "sess-gone"), "open");

    let start = env.start_session(&wt, "sess-new").success();
    assert!(!start.stdout.contains("STOP"), "{}", start.stdout);
    env.agent_edit(&wt, "a.py", "a = 1\n", "sess-new");
    env.agent_commit(&wt, "sess-new", "Add a").success();
    assert!(env.trailers(&wt, "HEAD").iter().any(|t| t == "Prov-Session: claude:sess-new"));
    env.end_session(&wt, "sess-new");

    // The stale session is still Open on the branch.
    let merge = env.try_git(&repo, ["merge", &branch("stale")]).failure();
    assert!(merge.stderr.contains("claude:sess-gone is open in the worktree"), "{}", merge.stderr);
    env.git(&repo, ["merge", "--abort"]);
    env.end_session(&wt, "sess-gone");
    env.try_git(&repo, ["merge", &branch("stale")]).success();
}
