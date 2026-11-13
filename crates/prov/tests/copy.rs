//! `prov cp`: the plan, the replay, the copy guard, the `Copied-From`
//! trailers and the deferred check.

// A panic in a test is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::Env;
use serde_json::Value;

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

/// A repository after `prov init`, with the files committed.
fn dest_repo(env: &Env, name: &str) -> PathBuf {
    let repo = env.plain_repo(name);
    env.install_gitai_integration();
    env.prov(&repo, ["init"]).success();
    env.git(&repo, ["add", "-A"]);
    env.git(&repo, ["commit", "-q", "-m", "Set up prov"]);
    repo
}

/// An edit by an AI session with a model: the lines that `content` adds to
/// the file belong to the session.
fn ai_edit(env: &Env, repo: &Path, file: &str, content: &str, tool: &str, model: &str, session: &str) {
    let path = repo.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    env.agent_checkpoint(repo, &serde_json::json!({"type": "human", "repo_working_dir": repo, "will_edit_filepaths": [path]}));
    std::fs::write(&path, content).unwrap();
    env.agent_checkpoint(
        repo,
        &serde_json::json!({"type": "ai_agent", "repo_working_dir": repo, "edited_filepaths": [path],
            "agent_name": tool, "model": model, "conversation_id": session}),
    );
}

/// A source repository with a directory of files of mixed authorship, in one commit.
fn source_repo(env: &Env) -> PathBuf {
    let src = env.plain_repo("source");
    // Lines of a human, then of two AI sessions between them.
    env.human_edit(&src, "lib/mixed.py", "import os\n\ndef main():\n    pass\n");
    ai_edit(env, &src, "lib/mixed.py", "import os\nimport sys\n\ndef main():\n    pass\n\ndef helper():\n    return 1\n", "claude", "m1", "s1");
    ai_edit(
        env,
        &src,
        "lib/mixed.py",
        "import os\nimport sys\n\ndef main():\n    pass\n\ndef helper():\n    return 1\n\ndef other():\n    return 2",
        "cursor",
        "m2",
        "s2",
    );
    ai_edit(env, &src, "lib/ai.py", "x = 1\ny = 2\n", "claude", "m1", "s1");
    env.human_edit(&src, "lib/deep/human.txt", "written by a person\nsecond line\n");
    env.human_edit(&src, "lib/tool.sh", "#!/bin/sh\necho tool\n");
    std::fs::set_permissions(src.join("lib/tool.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(src.join("lib/logo.bin"), [0u8, 1, 2, 0, 255, 10, 0]).unwrap();
    std::os::unix::fs::symlink("ai.py", src.join("lib/link")).unwrap();
    std::fs::write(src.join("lib/empty.txt"), "").unwrap();
    env.git(&src, ["add", "-A"]);
    env.git(&src, ["commit", "-q", "-m", "Add lib"]);
    env.await_notes(&src);
    src
}

fn copies(repo: &Path) -> Vec<Value> {
    let dir = repo.join(".git/provenance/copies");
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .map(|e| json(&std::fs::read_to_string(e.path()).unwrap()))
        .collect()
}

/// Waits until the deferred check has a result for every committed file.
#[track_caller]
fn wait_checked(repo: &Path) -> Vec<Value> {
    let begin = Instant::now();
    loop {
        let records = copies(repo);
        let waiting = records
            .iter()
            .flat_map(|r| r["files"].as_array().unwrap().clone())
            .any(|f| f["commit"].is_string() && f["check"].is_null());
        if !waiting {
            return records;
        }
        assert!(begin.elapsed() < Duration::from_secs(90), "the copy check did not finish: {records:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `(path, author)` of each added range of a commit, in line order.
fn line_authors(env: &Env, repo: &Path, commit: &str, path: &str) -> Vec<(u64, u64, String)> {
    env.await_notes(repo);
    let out = env.run(repo, "git-ai", ["diff", commit, "--json"], &[], None).success();
    let diff = json(&out.stdout);
    let mut rows = Vec::new();
    for hunk in diff["hunks"].as_array().unwrap() {
        if hunk["hunk_kind"] != "addition" || hunk["file_path"] != path {
            continue;
        }
        let author = if let Some(key) = hunk["session_id"].as_str() {
            let a = &diff["sessions"][key]["agent_id"];
            format!("{}/{}/{}", a["tool"].as_str().unwrap(), a["model"].as_str().unwrap(), a["id"].as_str().unwrap())
        } else if hunk["human_id"].is_string() {
            "human".to_string()
        } else {
            "untracked".to_string()
        };
        rows.push((hunk["start_line"].as_u64().unwrap(), hunk["end_line"].as_u64().unwrap(), author));
    }
    rows.sort();
    // Join ranges that touch and have one author.
    let mut joined: Vec<(u64, u64, String)> = Vec::new();
    for row in rows {
        match joined.last_mut() {
            Some(last) if last.2 == row.2 && last.1 + 1 == row.0 => last.1 = row.1,
            _ => joined.push(row),
        }
    }
    joined
}

#[test]
fn copies_a_directory_of_mixed_authorship_into_one_commit() {
    let Some(env) = Env::new() else { return };
    let src = source_repo(&env);
    let source_commit = env.git(&src, ["rev-parse", "HEAD"]);
    let repo = dest_repo(&env, "project");
    let base = env.git(&repo, ["rev-parse", "HEAD"]);

    // The human lines of the source have no author from the source: without
    // `--missing`, and with no terminal, the command names the ranges and stops.
    let stop = env.prov(&repo, ["cp", "--from", src.to_str().unwrap(), "lib", "third_party/lib"]).failure();
    assert!(stop.stderr.contains("third_party/lib/mixed.py line 1") && stop.stderr.contains("have no author"), "{}", stop.stderr);
    assert!(stop.stderr.contains("Nothing is written to the working tree"), "{}", stop.stderr);
    assert!(!repo.join("third_party").exists());

    // A dry run shows the plan and writes nothing.
    let dry = env.prov(&repo, ["cp", "--from", src.to_str().unwrap(), "lib", "third_party/lib", "--missing", "human", "--dry-run", "--json"]).success();
    let dry = json(&dry.stdout);
    assert_eq!(dry["applied"], false);
    assert_eq!(dry["files"].as_array().unwrap().len(), 7);
    assert!(!repo.join("third_party").exists() && copies(&repo).is_empty());

    let cp = env.prov(&repo, ["cp", "--from", src.to_str().unwrap(), "lib", "third_party/lib", "--missing", "human"]).success();
    assert!(cp.stdout.contains("Copied and staged 7 file(s)"), "{}", cp.stdout);
    // The summary: one line for each file, and the ranges of a file with more than one author.
    assert!(cp.stdout.contains("third_party/lib/ai.py  2 line(s)  ai claude/m1 (s1): 2"), "{}", cp.stdout);
    assert!(cp.stdout.contains("third_party/lib/mixed.py  11 line(s)"), "{}", cp.stdout);
    assert!(cp.stdout.contains("    6-8       ai claude/m1 (s1)"), "{}", cp.stdout);
    assert!(cp.stdout.contains("third_party/lib/logo.bin  binary") && cp.stdout.contains("third_party/lib/link  symbolic link"), "{}", cp.stdout);

    // Each file is equal to the source, byte for byte, with its mode.
    for file in ["mixed.py", "ai.py", "deep/human.txt", "tool.sh", "logo.bin", "empty.txt"] {
        assert_eq!(std::fs::read(repo.join("third_party/lib").join(file)).unwrap(), std::fs::read(src.join("lib").join(file)).unwrap(), "{file}");
    }
    assert_eq!(std::fs::read_link(repo.join("third_party/lib/link")).unwrap(), PathBuf::from("ai.py"));
    assert!(std::fs::metadata(repo.join("third_party/lib/tool.sh")).unwrap().permissions().mode() & 0o111 != 0);
    let staged = env.git(&repo, ["diff", "--cached", "--name-only"]);
    assert_eq!(staged.lines().count(), 7, "{staged}");
    assert_eq!(env.git(&src, ["ls-tree", "-r", &source_commit, "lib/tool.sh"]).split(' ').next(), Some("100755"));
    assert!(env.git(&repo, ["ls-files", "-s", "third_party/lib/tool.sh"]).starts_with("100755 "));

    // A second copy to the same destination is refused.
    let again = env.prov(&repo, ["cp", "--from", src.to_str().unwrap(), "lib/ai.py", "third_party/lib/ai.py", "--all", "human"]).failure();
    assert!(again.stderr.contains("the destination third_party/lib/ai.py exists"), "{}", again.stderr);

    // The commit holds the copy and an ordinary file.
    env.human_edit(&repo, "notes.txt", "my own file\n");
    env.git(&repo, ["add", "notes.txt"]);
    env.git(&repo, ["commit", "-q", "-m", "Copy lib from source"]);
    let trailers = env.trailers(&repo, "HEAD");
    assert_eq!(trailers.len(), 7, "{trailers:?}");
    let source = src.canonicalize().unwrap();
    assert!(
        trailers.contains(&format!("Copied-From: {}@{source_commit}:lib/mixed.py => third_party/lib/mixed.py", source.display())),
        "{trailers:?}"
    );

    // Each file agrees with its plan.
    let h = "human".to_string();
    let s1 = "claude/m1/s1".to_string();
    let s2 = "cursor/m2/s2".to_string();
    assert_eq!(
        line_authors(&env, &repo, "HEAD", "third_party/lib/mixed.py"),
        vec![(1, 1, h.clone()), (2, 2, s1.clone()), (3, 5, h.clone()), (6, 8, s1.clone()), (9, 11, s2)]
    );
    assert_eq!(line_authors(&env, &repo, "HEAD", "third_party/lib/ai.py"), vec![(1, 2, s1)]);
    assert_eq!(line_authors(&env, &repo, "HEAD", "third_party/lib/deep/human.txt"), vec![(1, 2, h.clone())]);
    assert_eq!(line_authors(&env, &repo, "HEAD", "third_party/lib/tool.sh"), vec![(1, 2, h.clone())]);
    assert_eq!(line_authors(&env, &repo, "HEAD", "notes.txt"), vec![(1, 1, h)]);

    // The deferred check agrees for every file and deletes the plan.
    let records = wait_checked(&repo);
    assert!(records.is_empty(), "{records:?}");
    env.prov(&repo, ["doctor"]).success();
    // The commit is a human commit with AI sessions in its note: the
    // `Copied-From` trailers permit them.
    let check = env.prov(&repo, ["check", &format!("{base}..HEAD")]);
    assert!(check.ok, "{}{}", check.stdout, check.stderr);
}

#[test]
fn a_plan_is_resolved_in_a_file_or_in_the_editor() {
    let Some(env) = Env::new() else { return };
    let src = source_repo(&env);
    let repo = dest_repo(&env, "project");
    let from = src.to_str().unwrap();

    // Only a human runs it, and not in an agent worktree.
    let agent = env.run(&repo, "prov", ["cp", "--from", from, "lib/ai.py", "ai.py"], &[("CLAUDECODE", "1")], None).failure();
    assert!(agent.stderr.contains("only a human"), "{}", agent.stderr);
    env.git(&repo, ["checkout", "-q", "-b", "agent/claude/x"]);
    let worktree = env.prov(&repo, ["cp", "--from", from, "lib/ai.py", "ai.py"]).failure();
    assert!(worktree.stderr.contains("agent worktree"), "{}", worktree.stderr);
    env.git(&repo, ["checkout", "-q", "main"]);

    // `--plan` writes the plan and stops.
    let plan_file = env.home().join("plan.toml");
    env.prov(&repo, ["cp", "--from", from, "lib/mixed.py", "lib/ai.py", "src/", "--plan", plan_file.to_str().unwrap()]).success();
    let plan = std::fs::read_to_string(&plan_file).unwrap();
    assert!(plan.contains("dest = \"src/mixed.py\"") && plan.contains("dest = \"src/ai.py\""), "{plan}");
    assert!(plan.contains("{ lines = \"1\",    author = \"?\" },"), "{plan}");
    assert!(plan.contains("{ lines = \"6-8\",  author = \"ai\", agent = \"claude\", model = \"m1\", session = \"s1\" },"), "{plan}");
    assert!(!repo.join("src").exists());

    // An unresolved plan is not applied.
    let unresolved = env.prov(&repo, ["cp", "--apply", plan_file.to_str().unwrap()]).failure();
    assert!(unresolved.stderr.contains("src/mixed.py lines 3-5"), "{}", unresolved.stderr);
    // One session with two models fails.
    std::fs::write(&plan_file, plan.replace("author = \"?\"", "author = \"ai\", agent = \"claude\", model = \"m9\", session = \"s1\"")).unwrap();
    let models = env.prov(&repo, ["cp", "--apply", plan_file.to_str().unwrap()]).failure();
    assert!(models.stderr.contains("two models"), "{}", models.stderr);
    // Ranges that do not cover the file fail.
    std::fs::write(&plan_file, plan.replace("{ lines = \"1\",    author = \"?\" },", "")).unwrap();
    let cover = env.prov(&repo, ["cp", "--apply", plan_file.to_str().unwrap()]).failure();
    assert!(cover.stderr.contains("no range has the lines 1-1"), "{}", cover.stderr);
    assert!(!repo.join("src").exists());

    // The plan with authors: line 1 to an AI with no session (it gets the plan ID), the others to the human.
    let resolved = plan
        .replace("{ lines = \"1\",    author = \"?\" }", "{ lines = \"1\", author = \"ai\", agent = \"codex\" }")
        .replace("author = \"?\"", "author = \"human\"");
    std::fs::write(&plan_file, resolved).unwrap();
    let applied = env.prov(&repo, ["cp", "--apply", plan_file.to_str().unwrap(), "--json"]).success();
    let applied = json(&applied.stdout);
    assert_eq!(applied["applied"], true);
    let id = applied["id"].as_str().unwrap().to_string();
    env.git(&repo, ["commit", "-q", "-m", "Copy two files"]);
    let rows = line_authors(&env, &repo, "HEAD", "src/mixed.py");
    assert_eq!(rows[0], (1, 1, format!("codex/unknown/{id}")));
    assert_eq!(rows[1], (2, 2, "claude/m1/s1".to_string()));
    assert_eq!(rows[2], (3, 5, "human".to_string()));
    assert_eq!(wait_checked(&repo), [] as [serde_json::Value; 0]);

    // At a terminal, an unresolved plan opens in the editor.
    let editor = env.home().join("editor.sh");
    std::fs::write(&editor, "#!/bin/sh\nsed -i 's/author = \"?\"/author = \"human\"/' \"$1\"\n").unwrap();
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();
    let edited = env.prov_tty_env(&repo, ["cp", "--from", from, "lib/deep", "deep"], "", &[("GIT_EDITOR", editor.to_str().unwrap())]);
    assert!(edited.ok, "{}{}", edited.stdout, edited.stderr);
    assert!(edited.stdout.contains("deep/human.txt  2 line(s)  human: 2"), "{}", edited.stdout);
    assert!(repo.join("deep/human.txt").is_file());
    // An editor that changes nothing: the command stops and keeps the plan file.
    let kept = env.prov_tty(&repo, ["cp", "--from", from, "lib/tool.sh", "bin/"], "").failure();
    assert!(kept.stderr.contains("bin/tool.sh lines 1-2") && kept.stderr.contains("prov cp --apply"), "{}", kept.stderr);
    assert!(!repo.join("bin").exists());

    // `--all ai` ignores the source's notes.
    env.prov(&repo, ["cp", "--from", from, "lib/tool.sh", "bin/", "--all", "ai", "--agent", "aider", "--model", "g1", "--session", "old-1"]).success();
    env.git(&repo, ["commit", "-q", "-m", "Copy more"]);
    assert_eq!(line_authors(&env, &repo, "HEAD", "bin/tool.sh"), vec![(1, 2, "aider/g1/old-1".to_string())]);
    assert_eq!(line_authors(&env, &repo, "HEAD", "deep/human.txt"), vec![(1, 2, "human".to_string())]);
    assert_eq!(wait_checked(&repo), [] as [serde_json::Value; 0]);
}

#[test]
fn the_guard_refuses_a_changed_copy_and_a_failed_check_is_replaced() {
    let Some(env) = Env::new() else { return };
    let src = source_repo(&env);
    let repo = dest_repo(&env, "project");
    let from = src.to_str().unwrap();

    env.prov(&repo, ["cp", "--from", from, "lib/ai.py", "ai.py"]).success();
    // A change to the copied file before the commit is refused.
    let copied = std::fs::read(repo.join("ai.py")).unwrap();
    std::fs::write(repo.join("ai.py"), "x = 1\ny = 2\nz = 3\n").unwrap();
    env.git(&repo, ["add", "ai.py"]);
    let refused = env.try_git(&repo, ["commit", "-q", "-m", "Copy"]).failure();
    assert!(refused.stderr.contains("different from the copied bytes"), "{}", refused.stderr);
    std::fs::write(repo.join("ai.py"), &copied).unwrap();
    env.git(&repo, ["add", "ai.py"]);

    // The stored plan gets another model than the replay recorded: the deferred check fails.
    let record_file = std::fs::read_dir(repo.join(".git/provenance/copies")).unwrap().flatten().next().unwrap().path();
    let record = std::fs::read_to_string(&record_file).unwrap();
    assert!(record.contains("\"model\": \"m1\""), "{record}");
    std::fs::write(&record_file, record.replace("\"model\": \"m1\"", "\"model\": \"wrong\"")).unwrap();
    env.git(&repo, ["commit", "-q", "-m", "Copy ai.py"]);
    assert_eq!(env.trailers(&repo, "HEAD").len(), 1);
    let records = wait_checked(&repo);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["failed"], true);
    assert_eq!(records[0]["files"][0]["check"], "failed");
    let doctor = env.prov(&repo, ["doctor"]).failure();
    assert!(doctor.stdout.contains("the copy check failed for ai.py"), "{}", doctor.stdout);

    // git-ai ignores some paths, for example `**/vendor/**`: a copy to one
    // gets no authorship, and the check reports it.
    env.prov(&repo, ["cp", "--from", from, "lib/ai.py", "vendor/ai.py"]).success();
    env.git(&repo, ["commit", "-q", "-m", "Copy to vendor"]);
    let records = wait_checked(&repo);
    let ignored: Vec<&Value> = records.iter().filter(|r| r["files"][0]["dest"] == "vendor/ai.py").collect();
    assert_eq!(ignored.len(), 1, "{records:?}");
    assert!(ignored[0]["files"][0]["detail"].as_str().unwrap().contains("git-ai ignores some paths"), "{records:?}");
    assert_eq!(ignored[0]["files"][0]["check"], "ignored");
    assert_eq!(ignored[0]["failed"], false);
    // A note of `prov doctor`, not a failure. The earlier failure of ai.py is still there.
    let doctor = env.prov(&repo, ["doctor"]).failure();
    assert!(doctor.stdout.contains("vendor/ai.py") && doctor.stdout.contains("has no line authorship"), "{}", doctor.stdout);
    std::fs::remove_file(repo.join(".git/provenance/copies").join(format!("{}.json", ignored[0]["id"].as_str().unwrap()))).unwrap();

    // Replacing the authorship of a file, as the specification describes it.
    env.git(&repo, ["rm", "-q", "ai.py"]);
    env.git(&repo, ["commit", "-q", "-m", "Remove ai.py"]);
    env.prov(&repo, ["cp", "--from", ".", "--rev", "HEAD~1", "ai.py", "ai.py", "--all", "human"]).success();
    env.git(&repo, ["commit", "-q", "-m", "Copy ai.py again, as my own"]);
    assert_eq!(line_authors(&env, &repo, "HEAD", "ai.py"), vec![(1, 2, "human".to_string())]);
    // The later copy agrees: the failure is cleared.
    let records = wait_checked(&repo);
    assert!(records.is_empty(), "{records:?}");
    env.prov(&repo, ["doctor"]).success();
}
