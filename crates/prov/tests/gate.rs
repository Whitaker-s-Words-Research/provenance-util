//! The merge gate, the rules around integration, and `prov check`.

mod common;

use std::path::Path;

use common::{branch, Env};

/// One archived session with one AI commit on `agent/claude/<name>`.
fn archived_branch(env: &Env, repo: &Path, name: &str, session: &str, file: &str) -> std::path::PathBuf {
    let wt = env.worktree(repo, name);
    env.start_session(&wt, session).success();
    env.agent_edit(&wt, file, "value = 1\n", session);
    env.agent_commit(&wt, session, &format!("Add {file}")).success();
    env.end_session(&wt, session);
    wt
}

/// Writes approvals into the merge message, as `prov approve --merge` will.
/// The trailers must stay in one block at the end of the message.
fn approve(env: &Env, repo: &Path, paths: &[&str]) {
    let mut args = vec!["interpret-trailers".to_string(), "--in-place".to_string()];
    for path in paths {
        args.push("--trailer".to_string());
        args.push(format!("Prov-Approved: {path} ack"));
    }
    args.push(repo.join(".git/MERGE_MSG").to_str().unwrap().to_string());
    env.git(repo, args);
}

/// A branch that changes the config and the tracked agent settings. The gate
/// must refuse it until the human approves, although every line is AI.
#[test]
fn protected_paths_need_approval() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = env.worktree(&repo, "weaken");
    env.start_session(&wt, "sess-w").success();
    // The agent removes the protection of its own settings ...
    env.agent_edit(
        &wt,
        ".provenance.toml",
        "required_version = \"=0.1.0\"\nhuman_branch = \"main\"\nagents = [\"claude\"]\nlogs_dir = \"llm-logs\"\nprotected = []\n",
        "sess-w",
    );
    // ... and changes them.
    env.agent_edit(&wt, ".claude/settings.json", "{\"hooks\": {}}\n", "sess-w");
    env.agent_edit(&wt, "src/ok.py", "ok = True\n", "sess-w");
    env.agent_commit(&wt, "sess-w", "Change the settings").success();
    env.end_session(&wt, "sess-w");

    // The gate reads the config from HEAD, not from the merged worktree.
    let merge = env.try_git(&repo, ["merge", &branch("weaken")]).failure();
    assert!(merge.stderr.contains(".provenance.toml: protected path changed"), "{}", merge.stderr);
    assert!(merge.stderr.contains(".claude/settings.json: protected path changed"), "{}", merge.stderr);
    assert!(!merge.stderr.contains("src/ok.py"), "{}", merge.stderr);

    // One approval is not enough.
    approve(&env, &repo, &[".provenance.toml"]);
    let again = env.try_git(&repo, ["commit", "--no-edit"]).failure();
    assert!(!again.stderr.contains(".provenance.toml: protected"), "{}", again.stderr);
    assert!(again.stderr.contains(".claude/settings.json: protected path changed"), "{}", again.stderr);

    approve(&env, &repo, &[".claude/settings.json"]);
    env.try_git(&repo, ["commit", "--no-edit"]).success();
    let base = env.git(&repo, ["rev-parse", "HEAD^1"]);
    // prov check reads the config at the first parent, so the paths are still protected there.
    env.await_notes(&repo);
    env.prov(&repo, ["check", &format!("{base}..HEAD")]).success();
}

#[test]
fn open_session_blocks_the_integration() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = env.worktree(&repo, "busy");
    env.start_session(&wt, "sess-b").success();
    env.agent_edit(&wt, "a.py", "a = 1\n", "sess-b");
    env.agent_commit(&wt, "sess-b", "Add a").success();

    let list = env.prov(&repo, ["check"]).success();
    assert!(list.stdout.contains(&branch("busy")) && list.stdout.contains("claude:sess-b (open)"), "{}", list.stdout);
    assert!(list.stdout.contains("doubtful lines: 0"), "{}", list.stdout);

    let merge = env.try_git(&repo, ["merge", &branch("busy")]).failure();
    assert!(merge.stderr.contains("session claude:sess-b is open"), "{}", merge.stderr);
    env.git(&repo, ["merge", "--abort"]);

    // A second session on the branch, open, with no commit of its own.
    env.end_session(&wt, "sess-b");
    env.start_session(&wt, "sess-idle").success();
    let merge = env.try_git(&repo, ["merge", &branch("busy")]).failure();
    assert!(merge.stderr.contains("claude:sess-idle is open in the worktree"), "{}", merge.stderr);
    env.git(&repo, ["merge", "--abort"]);

    env.end_session(&wt, "sess-idle");
    env.try_git(&repo, ["merge", &branch("busy")]).success();
    // Only the session that made a commit is in the trailers. The records of
    // both are deleted.
    let trailers = env.trailers(&repo, "HEAD");
    assert_eq!(trailers.iter().filter(|t| t.starts_with("Prov-Session:")).count(), 1, "{trailers:?}");
    assert!(env.record(&repo, "sess-b").is_none());
    assert!(env.record(&repo, "sess-idle").is_none());
}

#[test]
fn two_sessions_give_two_groups_in_start_order() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = archived_branch(&env, &repo, "pair", "sess-1", "one.py");
    std::thread::sleep(std::time::Duration::from_millis(1100)); // start times have a resolution of one second
    env.start_session(&wt, "sess-2").success();
    env.agent_edit(&wt, "two.py", "two = 2\n", "sess-2");
    env.agent_commit(&wt, "sess-2", "Add two").success();
    env.end_session(&wt, "sess-2");

    env.try_git(&repo, ["merge", &branch("pair")]).success();
    let keys: Vec<String> = env.trailers(&repo, "HEAD").iter().map(|t| t.split(' ').take(2).collect::<Vec<_>>().join(" ")).collect();
    assert_eq!(keys[0], "Prov-Session: claude:sess-1");
    assert!(keys[1].starts_with("Prov-Transcript-SHA256:"));
    assert_eq!(keys[2], "Prov-Session: claude:sess-2");
    assert!(keys[3].starts_with("Prov-Transcript-SHA256:"));
    assert!(keys[4].starts_with("Prov-Version:"));
    let base = env.git(&repo, ["rev-parse", "HEAD^1"]);
    env.await_notes(&repo);
    env.prov(&repo, ["check", &format!("{base}..HEAD")]).success();
}

#[test]
fn squash_is_refused_and_fast_forward_gets_a_warning() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    archived_branch(&env, &repo, "sq", "sess-sq", "sq.py");
    let base = env.git(&repo, ["rev-parse", "HEAD"]);

    // Squash. With `mergeoptions = --no-ff`, git itself refuses `--squash`.
    let plain = env.try_git(&repo, ["merge", "--squash", &branch("sq")]).failure();
    assert!(plain.stderr.contains("cannot be used together"), "{}", plain.stderr);
    // `--ff` overrides the option. Then prepare-commit-msg refuses the commit.
    env.git(&repo, ["merge", "-q", "--squash", "--ff", &branch("sq")]);
    let commit = env.try_git(&repo, ["commit", "-q", "-m", "Squash"]).failure();
    assert!(commit.stderr.contains("a squash of agent commits"), "{}", commit.stderr);
    assert!(commit.stderr.contains("git reset --hard"), "{}", commit.stderr);
    env.git(&repo, ["reset", "-q", "--hard"]);
    assert_eq!(env.git(&repo, ["rev-parse", "HEAD"]), base);
    // After the reset, an ordinary human commit works again.
    env.plain_write(&repo, "h.txt", "h\n");
    env.git(&repo, ["add", "-A"]);
    env.git(&repo, ["commit", "-q", "-m", "Human work"]);
    env.git(&repo, ["reset", "-q", "--hard", &base]);

    // Fast-forward: no hook can refuse it. post-merge prints how to undo it.
    let ff = env.try_git(&repo, ["merge", "--ff-only", &branch("sq")]).success();
    assert!(ff.stderr.contains("WARNING: this fast-forward put 1 agent commit(s)"), "{}", ff.stderr);
    assert!(ff.stderr.contains("git reset --keep ORIG_HEAD"), "{}", ff.stderr);
    // prov check refuses a first-parent commit that is not a human commit.
    let check = env.prov(&repo, ["check", &format!("{base}..HEAD")]).failure();
    assert!(check.stdout.contains("is not a human commit"), "{}", check.stdout);
    env.git(&repo, ["reset", "-q", "--keep", "ORIG_HEAD"]);
    assert_eq!(env.git(&repo, ["rev-parse", "HEAD"]), base);

    // A fast-forward of human commits gets no warning.
    env.git(&repo, ["branch", "feature"]);
    let other = env.home().join("feature-wt");
    env.git(&repo, ["worktree", "add", "-q", other.to_str().unwrap(), "feature"]);
    env.plain_write(&other, "f.txt", "f\n");
    env.git(&other, ["add", "-A"]);
    env.git(&other, ["commit", "-q", "-m", "Feature"]);
    let ff = env.try_git(&repo, ["merge", "--ff-only", "feature"]).success();
    assert!(!ff.stderr.contains("WARNING"), "{}", ff.stderr);
}

#[test]
fn a_merge_of_human_commits_is_not_an_integration() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let base = env.git(&repo, ["rev-parse", "HEAD"]);
    let other = env.home().join("feature-wt");
    env.git(&repo, ["worktree", "add", "-q", "-b", "feature", other.to_str().unwrap()]);
    env.plain_write(&other, "f.txt", "f\n");
    env.git(&other, ["add", "-A"]);
    env.git(&other, ["commit", "-q", "-m", "Feature"]);

    // The gate does not run: no trailers.
    env.try_git(&repo, ["merge", "feature"]).success();
    assert_eq!(env.git(&repo, ["log", "-1", "--format=%P"]).split(' ').count(), 2);
    assert!(env.trailers(&repo, "HEAD").is_empty());
    env.await_notes(&repo);
    env.prov(&repo, ["check", &format!("{base}..HEAD")]).success();
}

/// What a clone shows when the local hooks did not run.
#[test]
fn check_finds_what_bypassed_the_gate() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = env.worktree(&repo, "bypass");
    env.start_session(&wt, "sess-x").success();
    env.agent_edit(&wt, "a.py", "a = 1\n", "sess-x");
    env.human_edit(&wt, "notes.txt", "doubtful\n");
    env.agent_commit(&wt, "sess-x", "Add a").success();
    let agent_commit = env.git(&wt, ["rev-parse", "HEAD"]);
    env.end_session(&wt, "sess-x");
    let base = env.git(&repo, ["rev-parse", "HEAD"]);
    env.remove_shims(&repo);

    // A merge without the gate: this is how a forge merge appears.
    env.git(&repo, ["merge", "-q", "--no-ff", "-m", "Merge pull request #1", &branch("bypass")]);
    env.await_notes(&repo);
    let check = env.prov(&repo, ["check", &format!("{base}..HEAD")]).failure();
    assert!(check.stdout.contains("has no Prov-Version trailer"), "{}", check.stdout);
    assert!(check.stdout.contains("has no trailer group for the session claude:sess-x"), "{}", check.stdout);
    assert!(check.stdout.contains("has no Prov-Approved trailer for notes.txt"), "{}", check.stdout);
    let json = env.prov(&repo, ["check", &format!("{base}..HEAD"), "--json"]).failure();
    let json: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["failures"].as_array().unwrap().len(), 3);
    env.git(&repo, ["reset", "-q", "--hard", &base]);

    // A cherry-pick of an agent commit, committed by the human: a human
    // commit whose note names an AI session.
    env.git(&repo, ["cherry-pick", &agent_commit]);
    // The picked message keeps the agent's Prov-Session trailer, and the
    // author stays the agent: not a human commit.
    env.await_notes(&repo);
    let check = env.prov(&repo, ["check", &format!("{base}..HEAD")]).failure();
    assert!(check.stdout.contains("is not a human commit"), "{}", check.stdout);
    env.git(&repo, ["reset", "-q", "--hard", &base]);

    // A squash, committed by the human with a new message.
    env.git(&repo, ["merge", "-q", "--squash", "--ff", &branch("bypass")]);
    env.git(&repo, ["commit", "-q", "-m", "Squash the branch"]);
    env.await_notes(&repo);
    let check = env.prov(&repo, ["check", &format!("{base}..HEAD")]).failure();
    assert!(check.stdout.contains("gives lines of a.py to the AI session claude:sess-x"), "{}", check.stdout);
}

#[test]
fn version_mismatch_stops_the_gate_only() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    archived_branch(&env, &repo, "v", "sess-v", "v.py");
    let config = repo.join(".provenance.toml");
    let text = std::fs::read_to_string(&config).unwrap().replace("=0.1.0", "=9.9.9");
    std::fs::write(&config, text).unwrap();
    env.git(&repo, ["add", "-A"]);
    // Other hooks show a warning and continue.
    let commit = env.try_git(&repo, ["commit", "-q", "-m", "Require another version"]).success();
    assert!(commit.stderr.contains("does not satisfy required_version"), "{}", commit.stderr);

    let merge = env.try_git(&repo, ["merge", &branch("v")]).failure();
    assert!(merge.stderr.contains("does not satisfy required_version"), "{}", merge.stderr);
    env.git(&repo, ["merge", "--abort"]);
    let check = env.prov(&repo, ["check", "HEAD~1..HEAD"]).failure();
    assert!(check.stdout.contains("does not satisfy required_version"), "{}", check.stdout);
}
