//! Session states, binding rules and the agent hooks.

// A panic in a test is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

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
    assert_eq!(env.trailers(&repo, "HEAD"), [] as [std::string::String; 0]);

    // The same holds in a worktree on a branch that does not match `agent/**`.
    let other = env.repos().join("feature");
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
fn pre_commit_refuses_without_the_service() {
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
    // On the human branch, pre-commit refuses too: the commit would get no
    // note, and an amend would leave the note on the old commit.
    env.git(&repo, ["add", "-A"]);
    let human = env.try_git(&repo, ["commit", "-q", "-m", "Add h"]).failure();
    assert!(human.stderr.contains("refused: the git-ai service is not available"), "{}", human.stderr);
    let amend = env.try_git(&repo, ["commit", "-q", "--amend", "--no-edit"]).failure();
    assert!(amend.stderr.contains("refused: the git-ai service is not available"), "{}", amend.stderr);

    // SessionStart shows a warning and starts the service.
    let start = env.hook(&wt, "SessionStart", "sess-n").success();
    assert!(start.stdout.contains("the git-ai service is not available"), "{}", start.stdout);
    let begin = Instant::now();
    loop {
        let commit = env.agent_commit(&wt, "sess-n", "Add a");
        if commit.ok {
            break;
        }
        assert!(begin.elapsed() < Duration::from_secs(120), "the service did not start: {}", commit.stderr);
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The time limit of an agent hook in this worktree.
///
/// On a local file system it is the 100 ms of the specification. On a slow
/// file system (for example a 9p or network mount, where each `stat` costs
/// milliseconds), the git commands that a hook must run take longer than
/// that by themselves. The limit is then relative to them: the hook may add
/// 100 ms to three times their cost. So the test still fails when `prov`
/// adds a git process or a walk of the worktree.
fn hook_limit(env: &Env, worktree: &std::path::Path) -> Duration {
    let mut times: Vec<Duration> = (0..5)
        .map(|_| {
            let begin = Instant::now();
            env.git(worktree, ["rev-parse", "--show-toplevel", "--git-dir", "--git-common-dir"]);
            env.git(worktree, ["status", "--porcelain", "--untracked-files=all", "--no-renames"]);
            begin.elapsed()
        })
        .collect();
    times.sort();
    let git = times[times.len() / 2];
    if git < Duration::from_millis(25) {
        return Duration::from_millis(100);
    }
    let limit = Duration::from_millis(100) + git * 3;
    eprintln!("slow file system: the git commands of a hook take {git:?} here. The limit is {limit:?}, not 100 ms.");
    limit
}

/// An agent hook returns in less than 100 ms at the 95th percentile, on a
/// local file system. See [`hook_limit`] for a slow one.
#[test]
fn agent_hooks_are_fast() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = env.worktree(&repo, "latency");
    env.start_session(&wt, "sess-fast").success();
    for i in 0..20 {
        env.plain_write(&wt, &format!("gen/file{i}.txt"), "line\n");
    }
    let limit = hook_limit(&env, &wt);
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
        assert!(p95 < limit, "{event} takes {p95:?} at the 95th percentile; the limit is {limit:?}");
    }
    // SessionEnd only marks the session and starts a detached process.
    let begin = Instant::now();
    env.hook(&wt, "SessionEnd", "sess-fast").success();
    let end = begin.elapsed();
    eprintln!("SessionEnd: {end:?}");
    assert!(end < limit, "SessionEnd takes {end:?}; the limit is {limit:?}");
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

/// The author of the lines of `file` in a commit: git-ai's tool and session
/// for each added range.
fn authors_of(env: &Env, repo: &std::path::Path, commit: &str, file: &str) -> Vec<(String, String)> {
    env.authorship(repo, commit).into_iter().filter(|row| row.0 == file).map(|row| (row.1, row.2)).collect()
}

/// R-SWEEP-2. A session writes a file with no checkpoint and crashes. The
/// next session in the worktree must not get those lines: they are tainted,
/// no sweep takes them, and they reach the gate as doubtful lines.
#[test]
fn changes_of_a_crashed_session_are_tainted() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = env.worktree(&repo, "taint");
    env.start_session(&wt, "sess-a").success();
    env.plain_write(&wt, "from_a.txt", "written by a process of session a\n");
    // No Stop and no SessionEnd: the agent process is gone.
    env.kill_agent("sess-a");

    let start = env.start_session(&wt, "sess-b").success();
    assert!(!start.stdout.contains("STOP"), "{}", start.stdout);
    env.plain_write(&wt, "from_b.txt", "written by a process of session b\n");
    env.agent_commit(&wt, "sess-b", "Add two files").success();

    // The sweep gave session b its own file, and not the file of session a.
    assert_eq!(authors_of(&env, &wt, "HEAD", "from_b.txt"), vec![("claude".to_string(), "sess-b".to_string())]);
    // The lines of session a are AI lines of a session that nobody knows.
    let a = authors_of(&env, &wt, "HEAD", "from_a.txt");
    assert!(!a.is_empty() && a.iter().all(|(tool, id)| tool == "prov-unattributed" && id.starts_with("taint-")), "{a:?}");
    // After the commit, the file is no longer tainted: a new change to it is the occupant's.
    env.plain_write(&wt, "from_a.txt", "written by a process of session a\nand a line of session b\n");
    env.agent_commit(&wt, "sess-b", "Change the file").success();
    assert_eq!(authors_of(&env, &wt, "HEAD", "from_a.txt"), vec![("claude".to_string(), "sess-b".to_string())]);

    // At the gate, the lines of the crashed session are doubtful.
    env.end_session(&wt, "sess-b");
    env.hook(&wt, "SessionEnd", "sess-a").success();
    env.wait_state(&repo, "sess-a", "archived");
    let merge = env.try_git(&repo, ["merge", &branch("taint")]).failure();
    assert!(merge.stderr.contains("from_a.txt") && merge.stderr.contains("while the occupant of the agent worktree changed"), "{}", merge.stderr);
    assert!(!merge.stderr.contains("from_b.txt"), "{}", merge.stderr);
}

/// R-SWEEP-5. A session ends with no Stop. The sweep at `SessionEnd` gives it
/// its changes, so the next session's commit still records the right author.
#[test]
fn session_end_sweeps_for_the_session_that_ends() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = env.worktree(&repo, "endsweep");
    env.start_session(&wt, "sess-a").success();
    env.plain_write(&wt, "from_a.txt", "written by a process of session a\n");
    env.end_session(&wt, "sess-a");

    env.start_session(&wt, "sess-b").success();
    env.agent_commit(&wt, "sess-b", "Commit the work of session a").success();
    assert_eq!(authors_of(&env, &wt, "HEAD", "from_a.txt"), vec![("claude".to_string(), "sess-a".to_string())]);
}

/// R-SESS-2 and R-SESS-3. The archive step waits for the agent process, and
/// an archive process of an earlier generation changes nothing.
#[test]
fn an_archive_process_writes_only_for_its_generation() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = env.worktree(&repo, "generation");
    env.start_session(&wt, "sess-g").success();
    // SessionEnd arrives, and the agent process still runs: the session stays Closing.
    let pid = env.agent_pid("sess-g");
    env.hook_from(&wt, "SessionEnd", "sess-g", pid).success();
    std::thread::sleep(Duration::from_millis(600));
    let closing = env.record(&repo, "sess-g").unwrap();
    assert_eq!(closing["state"], "closing");
    let generation = closing["generation"].as_u64().unwrap();
    assert_eq!(generation, 2);

    // An event of the agent returns the session to Open: a new generation.
    env.hook(&wt, "UserPromptSubmit", "sess-g").success();
    assert_eq!(env.record(&repo, "sess-g").unwrap()["generation"], 3);
    env.hook_from(&wt, "SessionEnd", "sess-g", pid).success();
    assert_eq!(env.record(&repo, "sess-g").unwrap()["generation"], 4);

    // The archive process of generation 2 runs late. It writes nothing.
    env.prov(&wt, ["archive-session", "sess-g", "--generation", "2"]).success();
    assert_eq!(env.state(&repo, "sess-g"), "closing");

    // The agent exits. The archive process of generation 4 archives the session.
    env.kill_agent("sess-g");
    let archived = env.wait_state(&repo, "sess-g", "archived");
    assert_eq!(archived["generation"], 4);
}

/// The sweep in pre-commit gives the model of the transcript, also before the
/// first Stop. git-ai takes a model only from a checkpoint that records a
/// change, so the file here has no checkpoint before the sweep.
#[test]
fn a_commit_before_the_first_stop_gets_the_model_of_the_transcript() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = env.worktree(&repo, "model");
    env.start_session(&wt, "sess-m").success();
    // The SessionStart payload named one model; the transcript now names another.
    let transcript = env.transcript_path("sess-m");
    let text = std::fs::read_to_string(&transcript).unwrap();
    std::fs::write(
        &transcript,
        format!("{text}{}\n", r#"{"type":"assistant","message":{"role":"assistant","model":"claude-later-1","content":[]}}"#),
    )
    .unwrap();
    // A file that a script of the agent wrote. No Stop before the commit.
    env.plain_write(&wt, "m.py", "m = 1\n");
    env.agent_commit(&wt, "sess-m", "Add m").success();
    let rows = env.authorship(&wt, "HEAD");
    assert_eq!(rows, vec![("m.py".to_string(), "claude".to_string(), "sess-m".to_string(), "claude-later-1".to_string())]);
    assert_eq!(env.record(&repo, "sess-m").unwrap()["model"], "claude-later-1");
}

/// P4 of `ProvSessions`, as seen in a live run: the human closes a session
/// while its agent works. `prov session close` waits for the agent; the
/// agent's next event opens the session again, and the command says so.
#[test]
fn a_session_closed_while_its_agent_works_goes_on() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = env.worktree(&repo, "busy");
    env.start_session(&wt, "sess-busy").success();
    let close = std::thread::scope(|scope| {
        let closing = scope.spawn(|| env.prov(&repo, ["session", "close", "sess-busy"]));
        // The command waits for the agent; the agent sends its next event.
        let begin = Instant::now();
        while env.state(&repo, "sess-busy") != "closing" {
            assert!(begin.elapsed() < Duration::from_secs(60), "the session did not become closing");
            std::thread::sleep(Duration::from_millis(50));
        }
        env.hook(&wt, "Stop", "sess-busy").success();
        assert_eq!(env.state(&repo, "sess-busy"), "open");
        // The agent ends.
        env.hook(&wt, "SessionEnd", "sess-busy").success();
        closing.join().unwrap()
    });
    // The command exits 0 only if the session is archived by then (the
    // agent's own end can be that quick); either way it says what happened.
    assert!(close.stderr.contains("sent another event while prov waited"), "{}", close.stderr);
    let archived = env.wait_state(&repo, "sess-busy", "archived");
    assert_eq!(archived["generation"], 4);
}
