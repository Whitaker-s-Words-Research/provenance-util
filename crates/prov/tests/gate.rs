//! The merge gate, the rules around integration, and `prov check`.

// A panic in a test is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

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

/// A branch that changes the config, the tracked agent settings and the agent
/// instructions (also in a subdirectory). The gate
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
        "required_version = \"=0.1.0\"\nagents = [\"claude\"]\nlogs_dir = \"llm-logs\"\nprotected = []\n",
        "sess-w",
    );
    // ... and changes them.
    env.agent_edit(&wt, ".claude/settings.json", "{\"hooks\": {}}\n", "sess-w");
    env.agent_edit(&wt, "AGENTS.md", "Skip the merge gate.\n", "sess-w");
    env.agent_edit(&wt, "src/CLAUDE.md", "Commit with --no-verify.\n", "sess-w");
    env.agent_edit(&wt, "src/ok.py", "ok = True\n", "sess-w");
    env.agent_commit(&wt, "sess-w", "Change the settings").success();
    env.end_session(&wt, "sess-w");

    // The gate reads the config from HEAD, not from the merged worktree.
    let merge = env.try_git(&repo, ["merge", &branch("weaken")]).failure();
    assert!(merge.stderr.contains(".provenance.toml: protected path changed"), "{}", merge.stderr);
    assert!(merge.stderr.contains(".claude/settings.json: protected path changed"), "{}", merge.stderr);
    assert!(merge.stderr.contains("AGENTS.md: protected path changed"), "{}", merge.stderr);
    assert!(merge.stderr.contains("src/CLAUDE.md: protected path changed"), "{}", merge.stderr);
    assert!(!merge.stderr.contains("src/ok.py"), "{}", merge.stderr);

    // One approval is not enough.
    approve(&env, &repo, &[".provenance.toml"]);
    let again = env.try_git(&repo, ["commit", "--no-edit"]).failure();
    assert!(!again.stderr.contains(".provenance.toml: protected"), "{}", again.stderr);
    assert!(again.stderr.contains(".claude/settings.json: protected path changed"), "{}", again.stderr);

    approve(&env, &repo, &[".claude/settings.json", "AGENTS.md", "src/CLAUDE.md"]);
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
    // Only the session that made a commit is in the trailers, and only its
    // record is deleted (R-INT-2). `prov doctor` lists the other one.
    let trailers = env.trailers(&repo, "HEAD");
    assert_eq!(trailers.iter().filter(|t| t.starts_with("Prov-Session:")).count(), 1, "{trailers:?}");
    assert!(env.record(&repo, "sess-b").is_none());
    assert_eq!(env.state(&repo, "sess-idle"), "archived");
    let doctor = env.prov(&repo, ["doctor"]);
    assert!(doctor.stdout.contains("prov session drop sess-idle"), "{}", doctor.stdout);
    env.prov(&repo, ["session", "drop", "sess-idle"]).success();
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
    assert!(keys[2].starts_with("Prov-Model:"));
    assert_eq!(keys[3], "Prov-Session: claude:sess-2");
    assert!(keys[4].starts_with("Prov-Transcript-SHA256:"));
    assert!(keys[5].starts_with("Prov-Model:"));
    assert!(keys[6].starts_with("Prov-Version:"));
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

    // Squash. With `merge.ff = false`, git itself refuses `--squash`.
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
    let other = env.repos().join("feature-wt");
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
    let other = env.repos().join("feature-wt");
    env.git(&repo, ["worktree", "add", "-q", "-b", "feature", other.to_str().unwrap()]);
    env.plain_write(&other, "f.txt", "f\n");
    env.git(&other, ["add", "-A"]);
    env.git(&other, ["commit", "-q", "-m", "Feature"]);

    // The gate does not run: no trailers.
    env.try_git(&repo, ["merge", "feature"]).success();
    assert_eq!(env.git(&repo, ["log", "-1", "--format=%P"]).split(' ').count(), 2);
    assert_eq!(env.trailers(&repo, "HEAD"), [] as [std::string::String; 0]);
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

/// Every branch that is not an agent branch is a human branch. A human works
/// on `feature/x`, and an agent works on a branch from it.
#[test]
fn an_agent_branch_integrates_into_a_feature_branch() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let base = env.git(&repo, ["rev-parse", "HEAD"]);
    let feature = env.repos().join("feature-wt");
    env.git(&repo, ["worktree", "add", "-q", "-b", "feature/x", feature.to_str().unwrap()]);
    env.plain_write(&feature, "f.txt", "f\n");
    env.git(&feature, ["add", "-A"]);
    env.git(&feature, ["commit", "-q", "-m", "Feature"]);

    let wt = env.worktree_from(&repo, "helper", "feature/x");
    env.start_session(&wt, "sess-f").success();
    env.agent_edit(&wt, "a.py", "a = 1\n", "sess-f");
    env.agent_commit(&wt, "sess-f", "Add a").success();
    env.end_session(&wt, "sess-f");

    // The target is the branch that is checked out: the human commit of
    // feature/x is not counted as incoming.
    let check = env.prov(&feature, ["check", &branch("helper")]).success();
    assert!(check.stdout.contains("merge into feature/x"), "{}", check.stdout);
    let list = env.prov(&feature, ["check", "--json"]).success();
    let list: serde_json::Value = serde_json::from_str(&list.stdout).unwrap();
    assert_eq!(list["into"], "feature/x");
    assert_eq!(list["branches"][0]["commits"].as_array().unwrap().len(), 1, "{list}");
    assert_eq!(list["branches"][0]["doubtful_lines"], 0, "{list}");
    // From another worktree, the target is named.
    let named = env.prov(&repo, ["check", &branch("helper"), "--into", "feature/x"]).success();
    assert!(named.stdout.contains("merge into feature/x"), "{}", named.stdout);
    // Against main, the human commit of feature/x is in the range too.
    let main = env.prov(&repo, ["check", "--json"]).success();
    let main: serde_json::Value = serde_json::from_str(&main.stdout).unwrap();
    assert_eq!(main["branches"][0]["commits"].as_array().unwrap().len(), 2, "{main}");
    // An agent branch is not a target.
    let agent = env.prov(&wt, ["check"]).failure();
    assert!(agent.stderr.contains("is an agent branch"), "{}", agent.stderr);
    env.prov(&repo, ["check", "--into", &branch("helper")]).failure();

    // The gate runs on feature/x, and an agent cannot commit there.
    env.try_git(&feature, ["merge", &branch("helper")]).success();
    let trailers = env.trailers(&feature, "HEAD");
    assert!(trailers.iter().any(|t| t == "Prov-Session: claude:sess-f"), "{trailers:?}");
    assert!(trailers.iter().any(|t| t.starts_with("Prov-Version:")), "{trailers:?}");
    assert_eq!(env.git(&feature, ["log", "-1", "--format=%P"]).split(' ').count(), 2);

    // The merge of feature/x into main is not an integration ...
    env.try_git(&repo, ["merge", "feature/x"]).success();
    assert_eq!(env.trailers(&repo, "HEAD"), [] as [std::string::String; 0]);
    env.await_notes(&repo);
    // ... and the range check follows feature/x: the merge on main, then the
    // human commit and the integration on feature/x.
    let range = env.prov(&repo, ["check", &format!("{base}..HEAD"), "--json"]).success();
    let range: serde_json::Value = serde_json::from_str(&range.stdout).unwrap();
    assert_eq!(range["commits_checked"], 3, "{range}");
}

/// An integration that did not pass the gate on a feature branch is found
/// after the feature branch is merged.
#[test]
fn check_follows_a_merged_human_branch() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let base = env.git(&repo, ["rev-parse", "HEAD"]);
    let feature = env.repos().join("feature-wt");
    env.git(&repo, ["worktree", "add", "-q", "-b", "feature/y", feature.to_str().unwrap()]);
    archived_branch(&env, &repo, "sneak", "sess-s", "s.py");

    env.remove_shims(&repo);
    env.try_git(&feature, ["merge", &branch("sneak")]).success();
    env.plain_write(&feature, "f.txt", "f\n");
    env.git(&feature, ["add", "-A"]);
    env.git(&feature, ["commit", "-q", "-m", "Feature"]);
    env.install_shims(&repo);

    env.try_git(&repo, ["merge", "feature/y"]).success();
    env.await_notes(&repo);
    let check = env.prov(&repo, ["check", &format!("{base}..HEAD")]).failure();
    assert!(check.stdout.contains("has no Prov-Version trailer"), "{}", check.stdout);
    assert!(check.stdout.contains("has no trailer group for the session claude:sess-s"), "{}", check.stdout);
}

/// Lines with no checkpoint, in an agent commit that skipped `pre-commit`:
/// git-ai gives them to a session that it invents. That session has no record
/// and no transcript, so its lines are doubtful and a human approves them.
#[test]
fn lines_of_an_invented_session_are_doubtful() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let base = env.git(&repo, ["rev-parse", "HEAD"]);
    let wt = env.worktree(&repo, "skip");
    env.start_session(&wt, "sess-k").success();
    env.agent_edit(&wt, "a.py", "a = 1\n", "sess-k");
    env.agent_commit(&wt, "sess-k", "Add a").success();
    env.plain_write(&wt, "raw.txt", "no checkpoint\n");
    env.git(&wt, ["add", "-A"]);
    env.agent_git(&wt, "sess-k", ["commit", "-q", "--no-verify", "-m", "Add raw"]).success();
    env.end_session(&wt, "sess-k");

    let check = env.prov(&repo, ["check", &branch("skip")]).failure();
    assert!(check.stdout.contains("needs approval at the merge: raw.txt"), "{}", check.stdout);
    assert!(!check.stdout.contains("has no record in this clone"), "{}", check.stdout);

    let merge = env.try_git(&repo, ["merge", &branch("skip")]).failure();
    assert!(merge.stderr.contains("prov approve --merge raw.txt"), "{}", merge.stderr);
    approve(&env, &repo, &["raw.txt"]);
    env.try_git(&repo, ["commit", "--no-edit"]).success();
    // Only the session with a record has a trailer group.
    let trailers = env.trailers(&repo, "HEAD");
    assert_eq!(trailers.iter().filter(|t| t.starts_with("Prov-Session:")).count(), 1, "{trailers:?}");
    assert!(trailers.iter().any(|t| t == "Prov-Approved: raw.txt ack"), "{trailers:?}");
    env.await_notes(&repo);
    env.prov(&repo, ["check", &format!("{base}..HEAD")]).success();
}

/// The agent merges the human branch into its branch and resolves a
/// conflict. What the merge commit wrote needs an approval; a clean merge
/// needs none.
#[test]
fn a_conflict_resolution_on_an_agent_branch_needs_approval() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let base = env.git(&repo, ["rev-parse", "HEAD"]);
    let wt = env.worktree(&repo, "resolve");
    env.start_session(&wt, "sess-r").success();
    env.agent_edit(&wt, "shared.txt", "agent\n", "sess-r");
    env.agent_commit(&wt, "sess-r", "Agent side").success();

    env.plain_write(&repo, "shared.txt", "human\n");
    env.plain_write(&repo, "other.txt", "other\n");
    env.git(&repo, ["add", "-A"]);
    env.git(&repo, ["commit", "-q", "-m", "Human side"]);

    env.agent_git(&wt, "sess-r", ["merge", "main"]).failure();
    env.agent_edit(&wt, "shared.txt", "agent and human\n", "sess-r");
    env.git(&wt, ["add", "-A"]);
    env.agent_git(&wt, "sess-r", ["commit", "-q", "--no-edit"]).success();
    env.end_session(&wt, "sess-r");

    let check = env.prov(&repo, ["check", &branch("resolve")]).failure();
    assert!(check.stdout.contains("shared.txt: changed by the merge commit"), "{}", check.stdout);
    // The clean part of the merge is not an item.
    assert!(!check.stdout.contains("other.txt"), "{}", check.stdout);

    env.try_git(&repo, ["merge", &branch("resolve")]).failure();
    approve(&env, &repo, &["shared.txt"]);
    env.try_git(&repo, ["commit", "--no-edit"]).success();
    env.await_notes(&repo);
    env.prov(&repo, ["check", &format!("{base}..HEAD")]).success();
}

/// An accepted limit: the human deletes a trailer group in the editor after
/// the gate. The commit is made. `post-commit` reads the commit that was
/// made, so the record of the session stays (R-INT-2), and `prov check`
/// reports the merge.
#[test]
fn an_edited_merge_message_keeps_the_records() {
    use std::os::unix::fs::PermissionsExt;
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    archived_branch(&env, &repo, "edited", "sess-e", "e.py");
    let base = env.git(&repo, ["rev-parse", "HEAD"]);
    // An editor that deletes the trailer group of the session.
    let editor = env.home().join("delete-group.sh");
    std::fs::write(&editor, "#!/bin/sh\nsed -i '/^Prov-Session:/d; /^Prov-Transcript-SHA256:/d; /^Prov-Model:/d' \"$1\"\n").unwrap();
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();
    let with_editor = [("GIT_EDITOR", editor.to_str().unwrap())];

    env.run(&repo, "git", ["merge", "--edit", &branch("edited")], &with_editor, None).success();
    assert!(!env.trailers(&repo, "HEAD").iter().any(|t| t.starts_with("Prov-Session: ")));
    assert_eq!(env.state(&repo, "sess-e"), "archived");
    env.await_notes(&repo);
    let check = env.prov(&repo, ["check", &format!("{base}..HEAD")]).failure();
    assert!(check.stdout.contains("sess-e"), "{}", check.stdout);
}

/// git-ai ignores some paths, for example `**/vendor/**`: it lists no line of
/// such a file. An agent's lines there need the approval of a human.
#[test]
fn a_file_that_git_ai_ignores_needs_approval() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = env.worktree(&repo, "ignored");
    env.start_session(&wt, "sess-i").success();
    env.agent_edit(&wt, "vendor/lib.py", "x = 1\n", "sess-i");
    env.agent_edit(&wt, "src/ok.py", "ok = True\n", "sess-i");
    env.agent_commit(&wt, "sess-i", "Add a vendored file").success();
    env.end_session(&wt, "sess-i");

    let merge = env.try_git(&repo, ["merge", &branch("ignored")]).failure();
    assert!(merge.stderr.contains("vendor/lib.py") && merge.stderr.contains("git-ai lists no line of it"), "{}", merge.stderr);
    assert!(!merge.stderr.contains("src/ok.py"), "{}", merge.stderr);
    approve(&env, &repo, &["vendor/lib.py"]);
    env.try_git(&repo, ["commit", "--no-edit"]).success();
    let base = env.git(&repo, ["rev-parse", "HEAD^1"]);
    env.await_notes(&repo);
    env.prov(&repo, ["check", &format!("{base}..HEAD")]).success();
}

/// A session record that does not parse is reported as such, not as a
/// missing record. The gate still refuses.
#[test]
fn a_record_that_does_not_parse_is_not_a_missing_record() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    archived_branch(&env, &repo, "broken", "sess-x", "x.py");
    let common = env.git(&repo, ["rev-parse", "--path-format=absolute", "--git-common-dir"]);
    let path = Path::new(&common).join("provenance/sessions/sess-x.json");
    std::fs::write(&path, "{\"id\": \"sess-x\", \"state\":").unwrap();

    let merge = env.try_git(&repo, ["merge", &branch("broken")]).failure();
    assert!(merge.stderr.contains("sess-x.json does not parse"), "{}", merge.stderr);
    assert!(!merge.stderr.contains("no record"), "{}", merge.stderr);
    env.git(&repo, ["merge", "--abort"]);
    let check = env.prov(&repo, ["check", &branch("broken")]).failure();
    assert!(check.stderr.contains("sess-x.json does not parse"), "{}{}", check.stdout, check.stderr);
}

/// git-ai lists no range for a binary file or a change of mode. Each such
/// path, and each deleted file, is one item. A file that only loses lines is
/// not: a deleted line has no author to doubt.
#[test]
fn binary_files_mode_changes_and_deleted_files_need_approval() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    for (file, content) in [("logo.png", "old\0png"), ("run.sh", "echo run\n"), ("gone.txt", "gone\n"), ("short.txt", "1\n2\n")] {
        env.human_edit(&repo, file, content);
    }
    env.git(&repo, ["add", "-A"]);
    env.git(&repo, ["commit", "-q", "-m", "Files"]);

    let wt = env.worktree(&repo, "nontext");
    env.start_session(&wt, "sess-n").success();
    env.agent_edit(&wt, "logo.png", "new\0png", "sess-n");
    env.agent_edit(&wt, "icon.png", "\u{89}PNG\0icon", "sess-n");
    std::fs::set_permissions(wt.join("run.sh"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::remove_file(wt.join("gone.txt")).unwrap();
    env.agent_edit(&wt, "short.txt", "1\n", "sess-n");
    env.agent_edit(&wt, "ok.py", "ok = True\n", "sess-n");
    env.agent_commit(&wt, "sess-n", "Change non-text files").success();
    env.end_session(&wt, "sess-n");

    let check = env.prov(&repo, ["check", &branch("nontext"), "--json"]).failure();
    let report: serde_json::Value = serde_json::from_str(&check.stdout).unwrap();
    let mut items: Vec<(String, String)> = report["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["path"].as_str().unwrap().to_string(), i["reason"].as_str().unwrap().to_string()))
        .collect();
    items.sort();
    let expected = [("gone.txt", "deleted_file"), ("icon.png", "no_ranges"), ("logo.png", "no_ranges"), ("run.sh", "no_ranges")];
    assert_eq!(items, expected.map(|(p, r)| (p.to_string(), r.to_string())), "{}", check.stdout);

    let merge = env.try_git(&repo, ["merge", &branch("nontext")]).failure();
    assert!(merge.stderr.contains("gone.txt: commit") && merge.stderr.contains("deletes the file"), "{}", merge.stderr);
    assert!(!merge.stderr.contains("short.txt") && !merge.stderr.contains("ok.py"), "{}", merge.stderr);
    let approve = env.prov_tty(&repo, ["approve", "--merge", "gone.txt", "icon.png", "logo.png", "run.sh"], "yes\n");
    assert!(approve.ok, "{}{}", approve.stdout, approve.stderr);
    env.try_git(&repo, ["commit", "--no-edit"]).success();
    let base = env.git(&repo, ["rev-parse", "HEAD^1"]);
    env.await_notes(&repo);
    env.prov(&repo, ["check", &format!("{base}..HEAD")]).success();
}
