//! The commands: `prov init`, `prov doctor`, `prov start`, `prov finish`,
//! `prov session`, `prov approve` and `prov trace export`.

// A panic in a test is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};

use common::Env;
use serde_json::Value;

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

/// `prov init` in a plain repository, with the files committed.
fn init_repo(env: &Env, name: &str) -> PathBuf {
    let repo = env.plain_repo(name);
    env.install_gitai_integration();
    env.prov(&repo, ["init"]).success();
    env.git(&repo, ["add", "-A"]);
    env.git(&repo, ["commit", "-q", "-m", "Set up prov"]);
    repo
}

/// A session in a new worktree from `prov start --no-launch`, with one AI
/// commit. Returns the worktree and the branch.
fn started_session(env: &Env, repo: &Path, name: &str, session: &str) -> (PathBuf, String) {
    let start = env.prov(repo, ["start", "claude", "--no-launch", "--name", name, "--json"]).success();
    let start = json(&start.stdout);
    let wt = PathBuf::from(start["worktree"].as_str().unwrap());
    let branch = start["branch"].as_str().unwrap().to_string();
    env.start_session(&wt, session).success();
    env.agent_edit(&wt, &format!("{name}.py"), "value = 1\n", session);
    env.agent_commit(&wt, session, &format!("Add {name}")).success();
    (wt, branch)
}

#[test]
fn init_sets_up_a_clone_and_doctor_passes() {
    let Some(env) = Env::new() else { return };
    let repo = env.plain_repo("project");
    env.install_gitai_integration();
    // A hook and settings that exist before `prov init`.
    let hooks = env.hooks_dir(&repo);
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::write(hooks.join("pre-commit"), "#!/bin/sh\necho ran >> \"$(git rev-parse --git-dir)/local-hook-ran\"\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(hooks.join("pre-commit"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::create_dir_all(repo.join(".claude")).unwrap();
    std::fs::write(repo.join(".claude/settings.json"), r#"{"model": "opus", "env": {"KEEP": "1"}}"#).unwrap();

    // Before: doctor fails, and a dry run changes nothing.
    let before = env.prov(&repo, ["doctor"]).failure();
    assert!(before.stdout.contains("FAIL config") && before.stdout.contains("FAIL hooks"), "{}", before.stdout);
    let dry = env.prov(&repo, ["init", "--dry-run", "--json"]).success();
    assert_eq!(json(&dry.stdout)["dry_run"], true);
    assert!(!repo.join(".provenance.toml").exists());
    assert!(!hooks.join("pre-commit.local").exists());

    // An agent does not run `prov init`.
    let agent = env.run(&repo, "prov", ["init"], &[("CLAUDECODE", "1")], None).failure();
    assert!(agent.stderr.contains("for a human"), "{}", agent.stderr);
    // A hooks directory inside the worktree is refused.
    env.git(&repo, ["config", "core.hooksPath", ".githooks"]);
    let relative = env.prov(&repo, ["init"]).failure();
    assert!(relative.stderr.contains("core.hooksPath is the relative path"), "{}", relative.stderr);
    env.git(&repo, ["config", "--unset", "core.hooksPath"]);

    let init = env.prov(&repo, ["init"]).success();
    assert!(init.stdout.contains("Commit these files"), "{}", init.stdout);
    let config = std::fs::read_to_string(repo.join(".provenance.toml")).unwrap();
    assert!(config.contains("required_version = \"=0.1.0\"") && config.contains("agents = [\"claude\"]"), "{config}");
    assert!(!config.contains("human_branch"), "{config}");
    for name in ["pre-commit", "prepare-commit-msg", "post-commit", "post-merge"] {
        assert_eq!(std::fs::read_to_string(hooks.join(name)).unwrap(), format!("#!/bin/sh\nexec prov hook git {name} \"$@\"\n"));
    }
    assert!(hooks.join("pre-commit.local").is_file());
    let settings = json(&std::fs::read_to_string(repo.join(".claude/settings.json")).unwrap());
    assert_eq!(settings["model"], "opus");
    assert_eq!(settings["env"]["KEEP"], "1");
    assert_eq!(settings["env"]["GIT_AUTHOR_EMAIL"], "noreply@anthropic.com");
    assert_eq!(settings["env"]["GIT_CONFIG_VALUE_0"], "false");
    assert_eq!(settings["hooks"]["Stop"][0]["hooks"][0]["command"], "prov hook claude-code Stop");
    assert_eq!(env.git(&repo, ["config", "--local", "merge.ff"]), "false");
    assert_eq!(env.git(&repo, ["config", "--local", "pull.ff"]), "only");
    assert!(std::fs::read_to_string(repo.join(".gitignore")).unwrap().contains("/llm-logs/"));

    // Not committed yet: doctor says so.
    let uncommitted = env.prov(&repo, ["doctor"]).failure();
    assert!(uncommitted.stdout.contains(".provenance.toml is not tracked"), "{}", uncommitted.stdout);

    // The commit runs the shim, and the shim runs the hook that existed before.
    env.git(&repo, ["add", "-A"]);
    env.git(&repo, ["commit", "-q", "-m", "Set up prov"]);
    assert!(repo.join(".git/local-hook-ran").is_file(), "pre-commit.local did not run");
    let doctor = env.prov(&repo, ["doctor", "--json"]).success();
    let doctor = json(&doctor.stdout);
    assert_eq!(doctor["ok"], true, "{doctor}");
    assert_eq!(doctor["checks"].as_array().unwrap().len(), 9, "{doctor}");

    // A second run changes nothing.
    let again = json(&env.prov(&repo, ["init", "--json"]).success().stdout);
    assert!(again["actions"].as_array().unwrap().iter().all(|a| a["result"] == "kept"), "{again}");
    assert_eq!(env.git(&repo, ["status", "--porcelain"]), "");

    // A failing local hook makes the hook fail.
    std::fs::write(hooks.join("pre-commit.local"), "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::write(repo.join("x.txt"), "x\n").unwrap();
    env.git(&repo, ["add", "-A"]);
    let refused = env.try_git(&repo, ["commit", "-q", "-m", "x"]).failure();
    assert!(refused.stderr.contains("pre-commit.local failed"), "{}", refused.stderr);
    std::fs::remove_file(hooks.join("pre-commit.local")).unwrap();
    env.git(&repo, ["commit", "-q", "-m", "x"]);

    // What doctor finds.
    env.git(&repo, ["config", "merge.ff", "true"]);
    std::fs::write(hooks.join("post-merge"), "#!/bin/sh\n").unwrap();
    let broken = env.prov(&repo, ["doctor"]).failure();
    assert!(broken.stdout.contains("merge.ff is `true`"), "{}", broken.stdout);
    assert!(broken.stdout.contains("post-merge is not the shim"), "{}", broken.stdout);
    // The existing hook has other content, and `post-merge.local` does not exist: it is moved.
    env.prov(&repo, ["init"]).success();
    assert!(hooks.join("post-merge.local").is_file());
    env.prov(&repo, ["doctor"]).success();
}

/// A git-ai that reports `version` and passes every other command to the real git-ai.
fn gitai_reporting(env: &Env, version: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = env.home().join(format!("git-ai-{version}"));
    std::fs::write(
        &path,
        format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo {version}; exit 0; fi\nexec git-ai \"$@\"\n"),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// The supported git-ai range: older than the baseline fails; newer than the
/// tested version passes with a warning.
#[test]
fn doctor_checks_the_git_ai_version_range() {
    let Some(env) = Env::new() else { return };
    let repo = init_repo(&env, "project");
    let doctor = |version: &str| {
        let wrapper = gitai_reporting(&env, version);
        env.run(&repo, "prov", ["doctor", "--json"], &[("PROV_GIT_AI", wrapper.to_str().unwrap())], None)
    };
    let check = |out: &common::Out| {
        json(&out.stdout)["checks"].as_array().unwrap().iter().find(|c| c["name"] == "git-ai").cloned().unwrap()
    };

    let tested = doctor(prov_gitai::TESTED_VERSION).success();
    assert!(check(&tested).get("warnings").is_none(), "{}", tested.stdout);

    let newer = doctor("99.0.0").success();
    let git_ai = check(&newer);
    assert_eq!(git_ai["ok"], true, "{git_ai}");
    let warning = git_ai["warnings"][0].as_str().unwrap();
    assert!(warning.contains("99.0.0") && warning.contains(prov_gitai::TESTED_VERSION), "{warning}");

    let older = doctor("1.0.0").failure();
    assert_eq!(check(&older)["ok"], false, "{}", older.stdout);

    // The text output marks the warning.
    let wrapper = gitai_reporting(&env, "99.0.0");
    let text = env.run(&repo, "prov", ["doctor"], &[("PROV_GIT_AI", wrapper.to_str().unwrap())], None).success();
    assert!(text.stdout.contains("warning: git-ai is version 99.0.0"), "{}", text.stdout);
}

#[test]
fn start_session_approve_finish_and_trace() {
    let Some(env) = Env::new() else { return };
    let repo = init_repo(&env, "project");
    let base = env.git(&repo, ["rev-parse", "HEAD"]);

    // A dry run creates nothing.
    let dry = json(&env.prov(&repo, ["start", "claude", "--name", "dry", "--dry-run", "--json"]).success().stdout);
    assert!(!Path::new(dry["worktree"].as_str().unwrap()).exists());
    assert_eq!(dry["command"][0], "claude");
    env.prov(&repo, ["start", "claude", "--no-launch", "--name", "bad name"]).failure();
    env.prov(&repo, ["start", "claude", "--no-launch", "--branch", "feature/x"]).failure();
    env.prov(&repo, ["start", "other", "--no-launch"]).failure();
    // Only a human starts a session.
    let agent = env.run(&repo, "prov", ["start", "claude", "--no-launch", "--name", "self"], &[("CLAUDECODE", "1")], None).failure();
    assert!(agent.stderr.contains("only a human can start a session"), "{}", agent.stderr);
    // `prov.worktreesDir` in the git config of the clone, relative to the base directory.
    env.git(&repo, ["config", "prov.worktreesDir", "../elsewhere"]);
    let moved = json(&env.prov(&repo, ["start", "claude", "--name", "moved", "--dry-run", "--json"]).success().stdout);
    assert_eq!(Path::new(moved["worktree"].as_str().unwrap()).parent().unwrap(), env.repos().join("elsewhere"), "{moved}");
    // --path wins over it.
    let given = json(&env.prov(&repo, ["start", "claude", "--name", "given", "--dry-run", "--json", "--path", env.repos().join("p").to_str().unwrap()]).success().stdout);
    assert!(given["worktree"].as_str().unwrap().starts_with(env.repos().join("p").to_str().unwrap()), "{given}");
    env.git(&repo, ["config", "--unset", "prov.worktreesDir"]);

    let session = "sess-flow";
    let (wt, branch) = started_session(&env, &repo, "flow", session);
    assert!(branch.starts_with("agent/claude/") && branch.ends_with("-flow"), "{branch}");
    assert_eq!(wt.parent().unwrap(), env.repos().join("project.worktrees"));
    // The worktree has the tracked settings and config from the commit.
    assert!(wt.join(".claude/settings.json").is_file() && wt.join(".provenance.toml").is_file());
    // The branch exists now.
    env.prov(&repo, ["start", "claude", "--no-launch", "--name", "flow"]).failure();
    // A line that a human wrote in the agent worktree: doubtful.
    env.human_edit(&wt, "notes.txt", "from the human\n");
    env.agent_commit(&wt, session, "Add notes").success();

    // --- prov session -------------------------------------------------------
    let list = json(&env.prov(&repo, ["session", "list", "--json"]).success().stdout);
    assert_eq!(list[0]["id"], session);
    // The test session has no transcript, so no title.
    assert!(list[0]["title"].is_null(), "{list}");
    let text = env.prov(&repo, ["session", "list"]).success().stdout;
    assert!(text.starts_with(&format!("claude  {}  open  ", &session[..8])) && text.contains(&branch), "{text}");
    assert_eq!((&list[0]["state"], &list[0]["stale"], &list[0]["contended"]), (&json("\"open\""), &json("false"), &json("false")));
    let show = env.prov(&repo, ["session", "show", "sess-fl"]).success();
    assert!(show.stdout.contains(&branch) && show.stdout.contains("state:      open"), "{}", show.stdout);
    env.prov(&repo, ["session", "show", "nothing"]).failure();

    // --- prov finish refuses while the session is open -----------------------
    let refused = env.prov(&repo, ["finish", &branch]).failure();
    assert!(refused.stderr.contains("open or closing"), "{}", refused.stderr);
    assert!(wt.is_dir());
    env.prov(&repo, ["session", "archive", session, "--dry-run"]).success();
    assert_eq!(env.state(&repo, session), "open");
    // The agent still runs: the archive step waits for its process, and
    // says so. Here the agent exits one second later.
    let closed = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(1));
            assert_eq!(env.state(&repo, session), "closing");
            env.kill_agent(session);
        });
        env.prov(&repo, ["session", "archive", session, "--json"]).success()
    });
    assert!(closed.stderr.contains("waiting for the agent process"), "{}", closed.stderr);
    let closed = json(&closed.stdout);
    assert_eq!(closed["state"], "archived");
    let archive = PathBuf::from(closed["archive_file"].as_str().unwrap());
    assert_eq!(common::sha256_of_gunzipped(&archive), closed["archive_sha256"].as_str().unwrap());
    env.prov(&repo, ["session", "archive", session]).failure(); // already archived

    // --- The gate and prov approve --merge -----------------------------------
    let merge = env.try_git(&repo, ["merge", &branch]).failure();
    assert!(merge.stderr.contains("prov approve --merge notes.txt"), "{}", merge.stderr);
    // No terminal, an agent, a path with nothing to approve, a refusal at the prompt.
    let no_tty = env.prov(&repo, ["approve", "--merge", "notes.txt"]).failure();
    assert!(no_tty.stderr.contains("needs a terminal"), "{}", no_tty.stderr);
    let agent = env.run(&repo, "prov", ["approve", "--merge", "notes.txt"], &[("CLAUDECODE", "1")], None).failure();
    assert!(agent.stderr.contains("only a human can approve"), "{}", agent.stderr);
    let wrong = env.prov_tty(&repo, ["approve", "--merge", "flow.py"], "yes\n").failure();
    assert!(wrong.stderr.contains("nothing in this merge needs an approval for `flow.py`"), "{}", wrong.stderr);
    env.prov(&repo, ["approve", "notes.txt"]).failure(); // no mode
    env.prov_tty(&repo, ["approve", "--merge", "notes.txt"], "no\n").failure();
    env.try_git(&repo, ["commit", "--no-edit"]).failure();
    let dry = env.prov(&repo, ["approve", "--merge", "notes.txt", "--dry-run"]).success();
    assert!(dry.stdout.contains("are attributed to a human"), "{}", dry.stdout);

    let approved = env.prov_tty(&repo, ["approve", "--merge", "notes.txt"], "yes\n").success();
    assert!(approved.stdout.contains("Run: git commit"), "{}", approved.stdout);
    env.try_git(&repo, ["commit", "--no-edit"]).success();
    let trailers = env.trailers(&repo, "HEAD");
    assert!(trailers.contains(&"Prov-Approved: notes.txt ack".to_string()), "{trailers:?}");
    assert!(trailers.contains(&format!("Prov-Session: claude:{session}")), "{trailers:?}");
    assert!(env.record(&repo, session).is_none());
    env.await_notes(&repo);
    env.prov(&repo, ["check", &format!("{base}..HEAD")]).success();

    // --- prov finish ---------------------------------------------------------
    let finish = env.prov(&repo, ["finish", &branch]).success();
    assert!(finish.stdout.contains("The branch") && finish.stdout.contains("stays"), "{}", finish.stdout);
    // The branch is merged: `-d` deletes it. prov does not run the command.
    assert!(finish.stdout.contains(&format!("To delete it: git branch -d {branch}")), "{}", finish.stdout);
    assert!(!wt.exists());
    assert_eq!(env.git(&repo, ["branch", "--list", &branch]).trim(), branch);

    // --- prov trace export ---------------------------------------------------
    let trace = env.prov(&repo, ["trace", "export", &format!("{base}..HEAD")]).success();
    let records: Vec<Value> = trace.stdout.lines().map(json).collect();
    // The two agent commits and the merge have notes.
    assert!(records.len() >= 2, "{}", trace.stdout);
    let first = &records[0];
    assert_eq!(first["version"], "0.1");
    assert_eq!(first["vcs"]["type"], "git");
    assert_eq!(first["tool"], serde_json::json!({"name": "prov", "version": "0.1.0"}));
    assert_eq!(first["files"][0]["path"], "flow.py");
    assert_eq!(first["files"][0]["conversations"][0]["contributor"]["type"], "ai");
    // The agent's own hooks recorded the model `unknown`. The `Prov-Model`
    // trailer of the integration gives it.
    assert_eq!(first["files"][0]["conversations"][0]["contributor"]["model_id"], "anthropic/claude-fable-5-2");
    let notes = records.iter().find(|r| r["files"][0]["path"] == "notes.txt").expect("a record for notes.txt");
    assert_eq!(notes["files"][0]["conversations"][0]["contributor"]["type"], "human");
    // Two exports of the same range give identical bytes.
    assert_eq!(env.prov(&repo, ["trace", "export", &format!("{base}..HEAD")]).success().stdout, trace.stdout);
}

#[test]
fn finish_with_override_and_resume() {
    let Some(env) = Env::new() else { return };
    let repo = init_repo(&env, "project");
    // The agent is a stub that records how it was started.
    let config = std::fs::read_to_string(repo.join(".provenance.toml")).unwrap();
    std::fs::write(
        repo.join(".provenance.toml"),
        format!("{config}\n[agent.claude]\ncommand = [\"sh\", \"-c\", 'git var GIT_AUTHOR_IDENT > ../launched.txt; git config commit.gpgsign >> ../launched.txt; echo \"$@\" >> ../launched.txt', \"stub\"]\n"),
    )
    .unwrap();
    env.git(&repo, ["add", "-A"]);
    env.git(&repo, ["commit", "-q", "-m", "Stub agent"]);

    // A feature branch: the new agent branch starts from the branch that is checked out.
    env.git(&repo, ["switch", "-q", "-c", "feature/x"]);
    std::fs::write(repo.join("f.txt"), "f\n").unwrap();
    env.git(&repo, ["add", "-A"]);
    env.git(&repo, ["commit", "-q", "-m", "Feature"]);
    let feature = env.git(&repo, ["rev-parse", "HEAD"]);

    let launch = env.prov(&repo, ["start", "claude", "--name", "launch", "--", "--verbose"]).success();
    let wt = PathBuf::from(launch.stdout.trim());
    assert_eq!(env.git(&wt, ["merge-base", "HEAD", "feature/x"]), feature);
    let launched = std::fs::read_to_string(wt.parent().unwrap().join("launched.txt")).unwrap();
    let lines: Vec<&str> = launched.lines().collect();
    assert!(lines[0].starts_with("Claude <noreply@anthropic.com>"), "{launched}");
    assert_eq!(lines[1], "false", "{launched}");
    assert_eq!(lines[2], "--verbose", "{launched}");
    let branch = env.git(&wt, ["branch", "--show-current"]);

    let session = "sess-over";
    env.start_session(&wt, session).success();
    env.agent_edit(&wt, "a.py", "a = 1\n", session);
    env.agent_commit(&wt, session, "Add a").success();

    // --override closes the open session, then removes the worktree. The record stays.
    env.prov(&repo, ["finish", session]).failure();
    let dry = json(&env.prov(&repo, ["finish", session, "--override", "--dry-run", "--json"]).success().stdout);
    assert_eq!((&dry["removed"], &dry["closed"][0]), (&json("false"), &json("\"sess-over\"")));
    assert!(wt.is_dir());
    env.kill_agent(session); // the human ended the agent; no SessionEnd arrived
    let finish = env.prov(&repo, ["finish", session, "--override"]).success();
    assert!(finish.stdout.contains("not merged into feature/x"), "{}", finish.stdout);
    assert!(finish.stdout.contains(&format!("git branch -D {branch}")), "{}", finish.stdout);
    assert!(!wt.exists());
    assert_eq!(env.state(&repo, session), "archived");

    // --resume creates the worktree again on the session's branch, and gives the agent its resume option.
    let resume = env.prov(&repo, ["start", "claude", "--resume", session]).success();
    assert_eq!(PathBuf::from(resume.stdout.trim()), wt);
    assert_eq!(env.git(&wt, ["branch", "--show-current"]), branch);
    let launched = std::fs::read_to_string(wt.parent().unwrap().join("launched.txt")).unwrap();
    assert_eq!(launched.lines().last().unwrap(), format!("--resume {session}"));
    // A session with no record needs --branch.
    let unknown = env.prov(&repo, ["start", "claude", "--resume", "sess-none", "--no-launch"]).failure();
    assert!(unknown.stderr.contains("Also give --branch"), "{}", unknown.stderr);
    let by_branch = env.prov(&repo, ["start", "claude", "--resume", "sess-none", "--branch", &branch, "--no-launch"]).success();
    assert_eq!(PathBuf::from(by_branch.stdout.trim()), wt);

    // prov session drop deletes the record and keeps the archive.
    let archive = PathBuf::from(env.record(&repo, session).unwrap()["archive_file"].as_str().unwrap());
    env.prov(&repo, ["session", "drop", session, "--dry-run"]).success();
    assert!(env.record(&repo, session).is_some());
    env.prov(&repo, ["session", "drop", session]).success();
    assert!(env.record(&repo, session).is_none());
    assert!(archive.is_file());
}

#[test]
fn approve_human_and_ai_before_a_commit() {
    let Some(env) = Env::new() else { return };
    let repo = init_repo(&env, "project");
    let base = env.git(&repo, ["rev-parse", "HEAD"]);
    let approvals = repo.join(".git/provenance/approvals");
    let pending = || std::fs::read_dir(&approvals).map_or(0, std::iter::Iterator::count);

    // The output of a generator that the human ran.
    env.plain_write(&repo, "gen/table.txt", "1\n2\n");
    let wt = env.prov(&repo, ["start", "claude", "--no-launch", "--name", "a"]).success();
    let in_agent_worktree = env.prov_tty(Path::new(wt.stdout.trim()), ["approve", "--human", "x"], "yes\n").failure();
    assert!(in_agent_worktree.stderr.contains("not in an agent worktree"), "{}", in_agent_worktree.stderr);
    env.prov(&repo, ["approve", "--human", "gen/table.txt", "--model", "m"]).failure();
    env.prov_tty(&repo, ["approve", "--human", "gen/table.txt"], "yes\n").success();
    assert_eq!(pending(), 1);
    env.git(&repo, ["add", "-A"]);
    env.git(&repo, ["commit", "-q", "-m", "Add the table"]);
    assert_eq!(env.trailers(&repo, "HEAD"), ["Prov-Approved: gen/table.txt human"]);
    assert_eq!(pending(), 0, "the approval became a trailer");

    // Code pasted from an AI tool in a browser. From a subdirectory: the path is relative to it.
    env.plain_write(&repo, "src/pasted.py", "def f():\n    return 1\n");
    env.prov_tty(&repo.join("src"), ["approve", "--ai", "pasted.py", "--agent", "web-chat", "--model", "m1"], "yes\n").success();
    // A commit of other files leaves the approval pending.
    env.plain_write(&repo, "other.txt", "o\n");
    env.git(&repo, ["add", "other.txt"]);
    env.git(&repo, ["commit", "-q", "-m", "Other"]);
    assert_eq!(env.trailers(&repo, "HEAD"), [] as [std::string::String; 0]);
    assert_eq!(pending(), 1);
    env.git(&repo, ["add", "-A"]);
    env.git(&repo, ["commit", "-q", "-m", "Add pasted code"]);
    assert_eq!(env.trailers(&repo, "HEAD"), ["Prov-Approved: src/pasted.py ai"]);
    assert_eq!(pending(), 0);
    env.await_notes(&repo);
    let head = env.git(&repo, ["rev-parse", "HEAD"]);
    let authorship = env.authorship(&repo, &head);
    assert_eq!((authorship[0].1.as_str(), authorship[0].3.as_str()), ("web-chat", "m1"), "{authorship:?}");
    // A human commit with AI lines passes prov check because of the approval.
    env.prov(&repo, ["check", &format!("{base}..HEAD")]).success();

    // A change after the approval cancels it.
    env.plain_write(&repo, "late.txt", "one\n");
    env.prov_tty(&repo, ["approve", "--human", "late.txt"], "yes\n").success();
    env.plain_write(&repo, "late.txt", "one\ntwo\n");
    env.git(&repo, ["add", "-A"]);
    let commit = env.try_git(&repo, ["commit", "-q", "-m", "Late"]).success();
    assert!(commit.stderr.contains("late.txt changed after `prov approve --human`"), "{}", commit.stderr);
    assert_eq!(env.trailers(&repo, "HEAD"), [] as [std::string::String; 0]);
    assert_eq!(pending(), 0);
}

/// The `git-ai notes` check of `prov doctor`, from its JSON output.
fn notes_check(env: &Env, repo: &Path) -> Value {
    let doctor = json(&env.prov(repo, ["doctor", "--json"]).stdout);
    doctor["checks"].as_array().unwrap().iter().find(|c| c["name"] == "git-ai notes").cloned().expect("no git-ai notes check")
}

/// Starts the git-ai service of the environment again and waits until it runs.
fn start_service(env: &Env, repo: &Path) {
    env.run(repo, "git-ai", ["bg", "start"], &[], None);
    let begin = std::time::Instant::now();
    while !env.run(repo, "git-ai", ["bg", "status", "--repo", repo.to_str().unwrap()], &[], None).ok {
        assert!(begin.elapsed() < std::time::Duration::from_secs(120), "the git-ai service did not start");
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

#[test]
fn doctor_finds_commits_with_no_note_on_human_and_agent_branches() {
    let Some(env) = Env::new() else { return };
    let repo = init_repo(&env, "gaps");
    env.human_edit(&repo, "h.txt", "h\n");
    env.git(&repo, ["add", "-A"]);
    env.git(&repo, ["commit", "-q", "-m", "Add h"]);
    env.await_notes(&repo);
    let ok = notes_check(&env, &repo);
    assert_eq!(ok["ok"], true, "{ok}");
    let old = env.git(&repo, ["rev-parse", "HEAD"]);

    // While the service does not run: an amend on the human branch, and a
    // commit on an agent branch. pre-commit refuses both, so --no-verify.
    env.stop_service();
    env.git(&repo, ["commit", "-q", "--amend", "--no-verify", "-m", "Add h, amended"]);
    let amended = env.git(&repo, ["rev-parse", "HEAD"]);
    env.git(&repo, ["switch", "-q", "-c", "agent/claude/20261112-gaps"]);
    env.plain_write(&repo, "a.txt", "a\n");
    env.git(&repo, ["add", "-A"]);
    env.git(&repo, ["commit", "-q", "--no-verify", "-m", "Add a"]);
    let agent = env.git(&repo, ["rev-parse", "HEAD"]);
    env.git(&repo, ["switch", "-q", "main"]);
    start_service(&env, &repo);

    let check = notes_check(&env, &repo);
    assert_eq!(check["ok"], false, "{check}");
    let problems: Vec<&str> = check["problems"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
    assert_eq!(problems.len(), 2, "{check}");
    let human = problems.iter().find(|p| p.contains(&amended[..10])).expect("the amended commit");
    assert!(human.contains("on a human branch") && human.contains(&format!("Commit {}", &old[..10])), "{human}");
    let on_agent = problems.iter().find(|p| p.contains(&agent[..10])).expect("the agent commit");
    assert!(on_agent.contains("on an agent branch") && on_agent.contains("git-ai did not see the commit"), "{on_agent}");

    // The human accepts both gaps in the config.
    let config = std::fs::read_to_string(repo.join(".provenance.toml")).unwrap();
    std::fs::write(repo.join(".provenance.toml"), format!("{config}known_gaps = [\"{}\", \"{}\"]\n", &amended[..12], agent)).unwrap();
    env.git(&repo, ["commit", "-q", "-am", "Accept two gaps"]);
    env.await_notes(&repo);
    let accepted = notes_check(&env, &repo);
    assert_eq!(accepted["ok"], true, "{accepted}");
    assert_eq!(accepted["notes"].as_array().unwrap().len(), 2, "{accepted}");
}

/// The accepted limits warn where a person acts: R-SESS-3 (no agent process
/// ID) at close, and R-SWEEP-4 (processes in the worktree) at close, resume
/// and finish.
#[test]
fn close_resume_and_finish_warn_about_the_accepted_limits() {
    let Some(env) = Env::new() else { return };
    let repo = init_repo(&env, "project");
    let start = json(&env.prov(&repo, ["start", "claude", "--no-launch", "--name", "limits", "--json"]).success().stdout);
    let wt = PathBuf::from(start["worktree"].as_str().unwrap());
    let branch = start["branch"].as_str().unwrap().to_string();
    let session = "sess-limits";
    // `CLAUDE_PID=0`: prov does not know the agent process.
    env.write_transcript(session, &wt);
    env.hook_from(&wt, "SessionStart", session, 0).success();
    assert!(env.record(&repo, session).unwrap()["agent_pid"].is_null());

    // A development server that the agent started, still running.
    let mut server = std::process::Command::new("sleep")
        .arg("300")
        .current_dir(&wt)
        .stdin(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let listed = format!("{} sleep", server.id());

    let close = env.prov(&repo, ["session", "archive", session]).success();
    assert!(close.stderr.contains("does not know the process ID of the agent of session sess-limits"), "{}", close.stderr);
    assert!(close.stderr.contains("processes still run in the worktree"), "{}", close.stderr);
    assert!(close.stderr.contains(&listed), "{}", close.stderr);

    let resume = env.prov(&repo, ["start", "claude", "--resume", session, "--no-launch"]).success();
    assert!(resume.stderr.contains(&listed) && resume.stderr.contains("its sweep takes their changes"), "{}", resume.stderr);

    let finish = env.prov(&repo, ["finish", &branch]).success();
    assert!(finish.stderr.contains(&listed) && finish.stderr.contains("the worktree is removed now"), "{}", finish.stderr);

    server.kill().unwrap();
    server.wait().unwrap();

    // With no process in the worktree, and a known agent process, no warning.
    let (wt, _) = started_session(&env, &repo, "quiet", "sess-quiet");
    env.kill_agent("sess-quiet");
    let quiet = env.prov(&repo, ["session", "archive", "sess-quiet"]).success();
    assert!(!quiet.stderr.contains("warning"), "{}", quiet.stderr);
    assert!(wt.is_dir());
}

/// A worktree removed with `git worktree remove`, not `prov finish`, leaves
/// its owner record. `prov doctor` removes it.
#[test]
fn doctor_removes_the_record_of_a_removed_worktree() {
    let Some(env) = Env::new() else { return };
    let repo = init_repo(&env, "project");
    let (wt, _) = started_session(&env, &repo, "gone", "sess-gone");
    env.end_session(&wt, "sess-gone");
    let records = || std::fs::read_dir(repo.join(".git/provenance/worktrees")).map_or(0, Iterator::count);
    assert_eq!(records(), 1);
    env.git(&repo, ["worktree", "remove", "--force", wt.to_str().unwrap()]);
    let doctor = env.prov(&repo, ["doctor"]);
    assert!(doctor.stdout.contains("which no longer exists"), "{}", doctor.stdout);
    assert_eq!(records(), 0);
    // A second run has nothing to remove.
    assert!(!env.prov(&repo, ["doctor"]).stdout.contains("which no longer exists"));
}

#[test]
fn start_path_is_the_parent_and_completion_offers_branches() {
    let Some(env) = Env::new() else { return };
    let repo = init_repo(&env, "project");

    // --path names the directory that holds the worktree; the worktree's own
    // name comes from the branch.
    let parent = env.repos().join("elsewhere");
    let start = json(
        &env.prov(&repo, ["start", "claude", "--no-launch", "--name", "p1", "--path", parent.to_str().unwrap(), "--json"])
            .success()
            .stdout,
    );
    let branch = start["branch"].as_str().unwrap().to_string();
    let wt = PathBuf::from(start["worktree"].as_str().unwrap());
    assert_eq!(wt, parent.join(branch.strip_prefix("agent/").unwrap().replace('/', "-")));
    assert!(wt.is_dir());
    // A custom --branch also names the directory.
    let custom = json(
        &env.prov(&repo, ["start", "claude", "--no-launch", "--branch", "agent/claude/fix/x", "--path", "../elsewhere", "--json"])
            .success()
            .stdout,
    );
    assert_eq!(PathBuf::from(custom["worktree"].as_str().unwrap()), parent.join("claude-fix-x"));
    // A leading `~` is the home directory. The shell does not expand it in
    // `--path=~/dir`, so prov does.
    let tilde = json(
        &env.prov(&repo, ["start", "claude", "--no-launch", "--branch", "agent/claude/tilde", "--path=~/wts", "--json"])
            .success()
            .stdout,
    );
    assert_eq!(PathBuf::from(tilde["worktree"].as_str().unwrap()), env.home().join("wts/claude-tilde"));
    assert!(!repo.join("~").exists());

    // Tab completion: the candidates come from the repository.
    let complete = |words: &[&str]| {
        let mut args = vec!["--", "prov"];
        args.extend_from_slice(words);
        env.run(&repo, "prov", args, &[("COMPLETE", "fish")], None).success().stdout
    };
    let finish = complete(&["finish", ""]);
    assert!(finish.lines().any(|l| l.starts_with(&format!("{branch}\t"))), "{finish}");
    assert!(finish.lines().any(|l| l.starts_with("agent/claude/fix/x\t")), "{finish}");
    let prefixed = complete(&["finish", "agent/claude/f"]);
    assert_eq!(prefixed.lines().map(|l| l.split('\t').next().unwrap()).collect::<Vec<_>>(), ["agent/claude/fix/x"]);
    let from = complete(&["start", "claude", "--from", ""]);
    assert!(from.lines().any(|l| l == "main"), "{from}");
    assert_eq!(complete(&["start", ""]).lines().next(), Some("claude"));
    // The registration script for the shell's startup file.
    let script = env.run(&repo, "prov", Vec::<&str>::new(), &[("COMPLETE", "bash")], None).success().stdout;
    assert!(script.contains("complete") && script.contains("prov"), "{script}");
}

#[test]
fn completions_install_once_and_init_hints() {
    let Some(env) = Env::new() else { return };
    let repo = env.plain_repo("project");
    env.install_gitai_integration();
    let bash = [("SHELL", "/bin/bash")];
    let rc = env.home().join(".bashrc");
    std::fs::write(&rc, "alias ll='ls -l'").unwrap(); // no newline at the end

    // prov init changes nothing outside the clone. It says what is missing.
    let init = env.run(&repo, "prov", ["init"], &bash, None).success();
    assert!(init.stderr.contains("prov completions --install"), "{}", init.stderr);
    assert_eq!(std::fs::read_to_string(&rc).unwrap(), "alias ll='ls -l'");

    // With no flag: the line alone on standard output.
    let print = env.run(&repo, "prov", ["completions"], &bash, None).success();
    assert_eq!(print.stdout.trim(), "source <(COMPLETE=bash prov)");
    env.run(&repo, "prov", ["completions", "--install", "--dry-run"], &bash, None).success();
    assert_eq!(std::fs::read_to_string(&rc).unwrap(), "alias ll='ls -l'");

    let install = env.run(&repo, "prov", ["completions", "--install"], &bash, None).success();
    assert!(install.stdout.starts_with("added to"), "{}", install.stdout);
    let text = std::fs::read_to_string(&rc).unwrap();
    assert_eq!(text, "alias ll='ls -l'\n\n# Tab completion for prov\nsource <(COMPLETE=bash prov)\n");
    // A second install adds nothing, and init says nothing.
    let again = env.run(&repo, "prov", ["completions", "--install"], &bash, None).success();
    assert!(again.stdout.starts_with("already in"), "{}", again.stdout);
    assert_eq!(std::fs::read_to_string(&rc).unwrap(), text);
    let init = env.run(&repo, "prov", ["init"], &bash, None).success();
    assert!(!init.stderr.contains("completions"), "{}", init.stderr);

    // --shell overrides $SHELL; fish's file is created with its directory.
    let fish = json(&env.run(&repo, "prov", ["completions", "--shell", "fish", "--install", "--json"], &bash, None).success().stdout);
    assert_eq!(fish["added"], true);
    assert!(std::fs::read_to_string(env.home().join(".config/fish/config.fish")).unwrap().contains("COMPLETE=fish prov | source"));
    // An unknown shell: an error, not a guess.
    env.run(&repo, "prov", ["completions"], &[("SHELL", "/bin/tcsh")], None).failure();
}
