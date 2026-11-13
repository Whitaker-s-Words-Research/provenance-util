//! The git-ai contract: the behavior of git-ai that the rules of `prov` rest on.
//!
//! Each test checks one premise from "Appendix: test evidence" in SPEC.md,
//! with git-ai alone: the repositories have no `prov` hooks. The output is
//! read with the parsers of `prov-gitai`, so a change of the JSON format
//! fails here too.
//!
//! Run this suite against a new git-ai before raising
//! `prov_gitai::TESTED_VERSION`: `cargo test --test gitai_contract`. If a
//! test fails, a rule of the spec rests on a premise that is now false. Find
//! every rule that names the premise before changing any code.

// A panic in a test is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;

use common::Env;
use prov_gitai::{Author, CommitAuthorship};
use serde_json::json;

/// The authorship of HEAD, after git-ai has written its note.
fn head(env: &Env, repo: &Path) -> CommitAuthorship {
    env.await_notes(repo);
    let out = env.run(repo, "git-ai", ["diff", "HEAD", "--json"], &[], None).success();
    prov_gitai::parse_diff(&out.stdout).unwrap()
}

/// The authors of the added ranges of `path`.
fn authors(c: &CommitAuthorship, path: &str) -> Vec<Author> {
    c.added.iter().filter(|r| r.path == path).map(|r| r.author.clone()).collect()
}

fn ai(tool: &str, id: &str, model: &str) -> Author {
    Author::Ai(prov_gitai::AiSession { tool: tool.into(), id: id.into(), model: model.into() })
}

/// An `ai_agent` checkpoint through `agent-v1`, as the sweep and the taint make.
fn checkpoint_ai(env: &Env, repo: &Path, files: &[&str], tool: &str, model: &str, id: &str) {
    let paths: Vec<_> = files.iter().map(|f| repo.join(f)).collect();
    env.agent_checkpoint(
        repo,
        &json!({"type": "ai_agent", "repo_working_dir": repo, "edited_filepaths": paths,
            "agent_name": tool, "model": model, "conversation_id": id}),
    );
}

fn human_commit(env: &Env, repo: &Path, message: &str) {
    env.git(repo, ["add", "-A"]);
    env.git(repo, ["commit", "-q", "-m", message]);
}

/// `git-ai diff --json` gives each added range a `session_id` (AI) or a
/// `human_id` (human). `prov` reads authorship only from this JSON.
#[test]
fn diff_json_names_a_session_or_a_human_for_each_range() {
    let Some(env) = Env::new() else { return };
    let repo = env.plain_repo("diff");
    env.agent_edit(&repo, "ai.txt", "from the agent\n", "sess-d");
    env.human_edit(&repo, "human.txt", "from the human\n");
    human_commit(&env, &repo, "Both");
    let c = head(&env, &repo);
    assert_eq!(authors(&c, "ai.txt"), vec![ai("claude", "sess-d", "unknown")]);
    assert_eq!(authors(&c, "human.txt"), vec![Author::Human]);
}

/// A file with no checkpoint, in a commit with the agent identity that also
/// has a file with an `ai_agent` checkpoint: git-ai gives both files to the
/// AI session. This is why the taint is a checkpoint, not a skipped file (R-SWEEP-2).
#[test]
fn a_file_with_no_checkpoint_joins_the_ai_session_of_an_agent_commit() {
    let Some(env) = Env::new() else { return };
    let repo = env.plain_repo("fallback-agent");
    env.plain_write(&repo, "plain.txt", "no checkpoint\n");
    checkpoint_ai_after_write(&env, &repo, "ai.txt", "sess-f");
    env.git(&repo, ["add", "-A"]);
    env.agent_git(&repo, "sess-f", ["commit", "-q", "-m", "Agent"]).success();
    let c = head(&env, &repo);
    assert_eq!(authors(&c, "plain.txt"), vec![ai("claude", "sess-f", "m-1")]);
    assert_eq!(authors(&c, "ai.txt"), vec![ai("claude", "sess-f", "m-1")]);
}

/// The same in a commit by a human: the file with no checkpoint is
/// untracked, an addition with no `session_id` and no `human_id`. The gate
/// lists such lines as doubtful.
#[test]
fn a_file_with_no_checkpoint_is_untracked_in_a_human_commit_with_ai_lines() {
    let Some(env) = Env::new() else { return };
    let repo = env.plain_repo("fallback-human");
    env.plain_write(&repo, "plain.txt", "no checkpoint\n");
    checkpoint_ai_after_write(&env, &repo, "ai.txt", "sess-u");
    human_commit(&env, &repo, "Human");
    let c = head(&env, &repo);
    assert_eq!(authors(&c, "plain.txt"), vec![Author::Untracked]);
    assert_eq!(authors(&c, "ai.txt"), vec![ai("claude", "sess-u", "m-1")]);
}

/// The taint checkpoint (R-SWEEP-2): an `ai_agent` checkpoint under the tool
/// `prov-unattributed` and a new conversation ID gives the file that session,
/// in a commit by a human too.
#[test]
fn the_taint_checkpoint_gives_the_file_its_own_session() {
    let Some(env) = Env::new() else { return };
    let repo = env.plain_repo("taint");
    env.plain_write(&repo, "tainted.txt", "occupant changed\n");
    checkpoint_ai(&env, &repo, &["tainted.txt"], prov_gitai::UNATTRIBUTED_TOOL, "unknown", "taint-1");
    checkpoint_ai_after_write(&env, &repo, "ai.txt", "sess-t");
    human_commit(&env, &repo, "Human");
    let c = head(&env, &repo);
    let tainted = authors(&c, "tainted.txt");
    assert_eq!(tainted.len(), 1, "{tainted:?}");
    let Author::Ai(session) = &tainted[0] else { panic!("tainted.txt is {tainted:?}") };
    assert_eq!((session.tool.as_str(), session.id.as_str()), (prov_gitai::UNATTRIBUTED_TOOL, "taint-1"));
}

/// A checkpoint of type `human` just before the taint checkpoint takes the
/// content as it is, and the taint then records nothing. So the taint has
/// no `human` checkpoint before it.
#[test]
fn a_human_checkpoint_before_the_taint_leaves_the_taint_empty() {
    let Some(env) = Env::new() else { return };
    let repo = env.plain_repo("taint-after-human");
    env.plain_write(&repo, "tainted.txt", "occupant changed\n");
    env.agent_checkpoint(
        &repo,
        &json!({"type": "human", "repo_working_dir": repo, "will_edit_filepaths": [repo.join("tainted.txt")]}),
    );
    checkpoint_ai(&env, &repo, &["tainted.txt"], prov_gitai::UNATTRIBUTED_TOOL, "unknown", "taint-2");
    checkpoint_ai_after_write(&env, &repo, "ai.txt", "sess-h");
    human_commit(&env, &repo, "Human");
    let c = head(&env, &repo);
    let tainted = authors(&c, "tainted.txt");
    assert!(
        !tainted.iter().any(|a| matches!(a, Author::Ai(s) if s.tool == prov_gitai::UNATTRIBUTED_TOOL)),
        "the taint recorded the file: {tainted:?}"
    );
}

/// git-ai takes a model only from a checkpoint that records a change ("Agent
/// model"). A checkpoint with a model on a file that has no new change keeps
/// the model `unknown`; with a change in another file, the session gets the model.
#[test]
fn the_model_comes_only_from_a_checkpoint_that_records_a_change() {
    let Some(env) = Env::new() else { return };

    let repo = env.plain_repo("model-no-change");
    env.agent_edit(&repo, "a.txt", "a\n", "sess-m");
    checkpoint_ai(&env, &repo, &["a.txt"], "claude", "m-later", "sess-m");
    env.git(&repo, ["add", "-A"]);
    env.agent_git(&repo, "sess-m", ["commit", "-q", "-m", "Agent"]).success();
    assert_eq!(authors(&head(&env, &repo), "a.txt"), vec![ai("claude", "sess-m", "unknown")]);

    let repo = env.plain_repo("model-change");
    env.agent_edit(&repo, "a.txt", "a\n", "sess-m");
    env.plain_write(&repo, "b.txt", "b\n");
    checkpoint_ai(&env, &repo, &["a.txt", "b.txt"], "claude", "m-later", "sess-m");
    env.git(&repo, ["add", "-A"]);
    env.agent_git(&repo, "sess-m", ["commit", "-q", "-m", "Agent"]).success();
    let c = head(&env, &repo);
    assert_eq!(authors(&c, "a.txt"), vec![ai("claude", "sess-m", "m-later")]);
    assert_eq!(authors(&c, "b.txt"), vec![ai("claude", "sess-m", "m-later")]);
}

/// The sweep joins the agent's session: a checkpoint with the same tool and
/// conversation ID as the agent's own hooks gives one session key.
#[test]
fn the_sweep_joins_the_session_of_the_agents_own_hooks() {
    let Some(env) = Env::new() else { return };
    let repo = env.plain_repo("join");
    env.agent_edit(&repo, "hooked.txt", "by a tool\n", "sess-j");
    env.plain_write(&repo, "swept.txt", "by a script\n");
    checkpoint_ai(&env, &repo, &["swept.txt"], "claude", "m-1", "sess-j");
    env.git(&repo, ["add", "-A"]);
    env.agent_git(&repo, "sess-j", ["commit", "-q", "-m", "Agent"]).success();
    let c = head(&env, &repo);
    assert_eq!(c.sessions.len(), 1, "{:?}", c.sessions);
    assert_eq!(authors(&c, "hooked.txt"), authors(&c, "swept.txt"));
}

/// The sweep in `pre-commit` runs just before the commit. Its checkpoint must
/// count for that commit, also for a file that had no checkpoint before.
/// Repeated, because an ordering problem shows only sometimes.
#[test]
fn a_checkpoint_just_before_the_commit_counts() {
    let Some(env) = Env::new() else { return };
    let repo = env.plain_repo("just-before");
    for i in 0..5 {
        let file = format!("f{i}.txt");
        env.plain_write(&repo, &file, &format!("{i}\n"));
        checkpoint_ai(&env, &repo, &[&file], "claude", "m-1", "sess-b");
        env.git(&repo, ["add", "-A"]);
        env.agent_git(&repo, "sess-b", ["commit", "-q", "-m", &file]).success();
        assert_eq!(authors(&head(&env, &repo), &file), vec![ai("claude", "sess-b", "m-1")], "commit {i}");
    }
}

/// Paths in git-ai's ignore list get no ranges at all. The gate lists such a
/// file as an item of its own.
#[test]
fn ignored_paths_have_no_ranges() {
    let Some(env) = Env::new() else { return };
    let repo = env.plain_repo("ignored");
    env.agent_edit(&repo, "vendor/v.txt", "vendored\n", "sess-i");
    env.agent_edit(&repo, "src.txt", "source\n", "sess-i");
    env.git(&repo, ["add", "-A"]);
    env.agent_git(&repo, "sess-i", ["commit", "-q", "-m", "Agent"]).success();
    let c = head(&env, &repo);
    assert!(authors(&c, "vendor/v.txt").is_empty(), "{:?}", c.added);
    assert_eq!(authors(&c, "src.txt").len(), 1);
}

/// `git-ai blame --json` lists only the AI lines of a file, and fails on an
/// empty file. `prov cp` inherits only AI sessions, and skips empty files.
#[test]
fn blame_lists_only_ai_lines_and_fails_on_an_empty_file() {
    let Some(env) = Env::new() else { return };
    let repo = env.plain_repo("blame");
    env.human_edit(&repo, "mixed.txt", "human 1\n");
    human_commit(&env, &repo, "Human");
    env.agent_edit(&repo, "mixed.txt", "human 1\nagent 2\n", "sess-l");
    env.plain_write(&repo, "empty.txt", "");
    env.git(&repo, ["add", "-A"]);
    env.agent_git(&repo, "sess-l", ["commit", "-q", "-m", "Agent"]).success();
    env.await_notes(&repo);

    let out = env.run(&repo, "git-ai", ["blame", "mixed.txt", "--json"], &[], None).success();
    let ranges = prov_gitai::parse_blame(&out.stdout).unwrap();
    assert_eq!(ranges.len(), 1, "{ranges:?}");
    assert_eq!((ranges[0].0, ranges[0].1, ranges[0].2.id.as_str()), (2, 2, "sess-l"));

    env.run(&repo, "git-ai", ["blame", "empty.txt", "--json"], &[], None).failure();
}

/// A new file with an `ai_agent` checkpoint (tool `claude`, model `m-1`) in
/// `session`: an AI file next to the file under test.
fn checkpoint_ai_after_write(env: &Env, repo: &Path, file: &str, session: &str) {
    env.plain_write(repo, file, &format!("{file}\n"));
    checkpoint_ai(env, repo, &[file], "claude", "m-1", session);
}
