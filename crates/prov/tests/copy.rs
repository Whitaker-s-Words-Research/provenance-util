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
    // The note ends with the command that removes it, on a line of its own.
    let lines: Vec<&str> = doctor.stdout.lines().collect();
    let at = lines.iter().position(|l| l.contains("vendor/ai.py")).unwrap();
    let command = lines[at..].iter().map(|l| l.trim()).find(|l| l.starts_with("rm ")).unwrap();
    assert!(command.contains(ignored[0]["id"].as_str().unwrap()), "{}", doctor.stdout);
    let out = env.run(&repo, "sh", ["-c", command], &[], None);
    assert!(out.ok, "{command}\n{}", out.stderr);
    let doctor = env.prov(&repo, ["doctor"]).failure();
    assert!(!doctor.stdout.contains("vendor/ai.py"), "{}", doctor.stdout);

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

/// An editor that needs the terminal: it checks that its standard input and
/// output are a terminal, and reads what the human types there.
#[test]
fn an_editor_that_needs_the_terminal() {
    let Some(env) = Env::new() else { return };
    let src = source_repo(&env);
    let repo = dest_repo(&env, "project");
    let editor = env.home().join("tty-editor.sh");
    std::fs::write(
        &editor,
        "#!/bin/sh\n[ -t 0 ] && [ -t 1 ] || { echo 'not a terminal' >&2; exit 1; }\nprintf 'author? '\nread answer\n\
         sed -i \"s/author = \\\"?\\\"/author = \\\"$answer\\\"/\" \"$1\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = env.prov_tty_env(&repo, ["cp", "--from", src.to_str().unwrap(), "lib/deep", "deep"], "human\n", &[("GIT_EDITOR", editor.to_str().unwrap())]);
    assert!(out.ok, "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("deep/human.txt  2 line(s)  human: 2"), "{}", out.stdout);
}

/// A clean filter in the source, as Git LFS uses: `prov cp` copies the blob,
/// the filtered content, not the file of the source's worktree. A filter of
/// the destination that changes the bytes at `git add` gives a note, and the
/// deferred check still agrees.
#[test]
fn copies_the_blob_of_a_filtered_file() {
    let Some(env) = Env::new() else { return };
    let src = env.plain_repo("filtered");
    std::fs::write(src.join(".gitattributes"), "*.dat filter=redact\n").unwrap();
    env.git(&src, ["config", "filter.redact.clean", "sed s/secret/REDACTED/"]);
    env.git(&src, ["config", "filter.redact.smudge", "cat"]);
    ai_edit(&env, &src, "data.dat", "key = secret\nother = 1\n", "claude", "m1", "s1");
    env.git(&src, ["add", "-A"]);
    env.git(&src, ["commit", "-q", "-m", "Data"]);
    env.await_notes(&src);
    assert_eq!(std::fs::read_to_string(src.join("data.dat")).unwrap(), "key = secret\nother = 1\n");

    let repo = dest_repo(&env, "project");
    env.prov(&repo, ["cp", "--from", src.to_str().unwrap(), "data.dat", "data.dat", "--missing", "human"]).success();
    assert_eq!(std::fs::read_to_string(repo.join("data.dat")).unwrap(), "key = REDACTED\nother = 1\n");

    // A filter in the destination that changes the bytes at `git add`.
    std::fs::write(repo.join(".gitattributes"), "*.txt filter=upper\n").unwrap();
    env.git(&repo, ["config", "filter.upper.clean", "tr a-z A-Z"]);
    env.git(&repo, ["config", "filter.upper.smudge", "cat"]);
    env.git(&repo, ["add", ".gitattributes"]);
    env.git(&repo, ["commit", "-q", "-m", "Attributes"]);
    let out = env.prov(&repo, ["cp", "--from", src.to_str().unwrap(), "data.dat", "copy.txt", "--missing", "human"]).success();
    eprintln!("{}", out.stdout);
    env.git(&repo, ["commit", "-q", "-m", "Copy"]);
    let records = wait_checked(&repo);
    eprintln!("copy records after the check: {records:?}");
}

/// A copy of 2,000 files: the time of the plan, the copy and the deferred
/// check. About two minutes, so it runs only on request:
/// `ci/dev.sh test -p prov --test copy -- --ignored --nocapture`.
#[test]
#[ignore = "slow: about two minutes"]
fn copies_two_thousand_files() {
    let Some(env) = Env::new() else { return };
    let src = env.plain_repo("many");
    for i in 0..2000 {
        env.plain_write(&src, &format!("lib/d{}/f{i}.txt", i % 20), &format!("line one of {i}\nline two\n"));
    }
    env.git(&src, ["add", "-A"]);
    env.git(&src, ["commit", "-q", "-m", "Many"]);
    env.await_notes(&src);
    let repo = dest_repo(&env, "project");

    let begin = Instant::now();
    let out = env.prov(&repo, ["cp", "--from", src.to_str().unwrap(), "lib", "lib", "--missing", "human", "--json"]);
    let copy = begin.elapsed();
    assert!(out.ok, "{}", out.stderr);
    let begin = Instant::now();
    env.git(&repo, ["commit", "-q", "-m", "Copy many"]);
    let commit = begin.elapsed();
    let begin = Instant::now();
    while !env.try_git(&repo, ["notes", "--ref=ai", "list", "HEAD"]).ok && begin.elapsed() < Duration::from_secs(120) {
        std::thread::sleep(Duration::from_millis(200));
    }
    let note = begin.elapsed();
    let records = wait_checked(&repo);
    let check = begin.elapsed();
    eprintln!("2,000 files: the note arrived after {note:?}");
    eprintln!("2,000 files: prov cp {copy:?}, git commit {commit:?}, deferred check {check:?}");
    let begin = Instant::now();
    env.run(&repo, "git-ai", ["diff", "HEAD", "--json"], &[], None);
    eprintln!("2,000 files: one git-ai diff of the commit {:?}", begin.elapsed());
    assert_eq!(records, [] as [Value; 0], "the check agrees for every file");
    assert_eq!(env.git(&repo, ["ls-files", "lib"]).lines().count(), 2000);
}

/// A source with Git LFS. The blob of an LFS file is a pointer; `prov cp`
/// copies the content from the source's local LFS store, with no line
/// authorship. With git-lfs missing, or the object missing from the store,
/// it refuses and copies nothing. LFS refuses to install its hooks over the
/// shims; with `--force` and `prov init` again, both run. A commit with an
/// LFS file once waited 60 seconds for the detached copy check, which held a
/// pipe of `git-lfs filter-process`.
#[test]
fn copies_from_a_source_with_git_lfs() {
    let Some(env) = Env::new() else { return };
    if !env.try_git(env.home(), ["lfs", "version"]).ok {
        eprintln!("SKIPPED: git-lfs is not installed");
        return;
    }
    let content = "the real content\nof a large file\n";
    let src = env.plain_repo("lfs");
    env.git(&src, ["lfs", "install", "--local"]);
    env.git(&src, ["lfs", "track", "*.bin"]);
    std::fs::write(src.join("big.bin"), content).unwrap();
    std::fs::write(src.join("gone.bin"), "an object that the store lost\n").unwrap();
    env.git(&src, ["add", "-A"]);
    env.git(&src, ["commit", "-q", "-m", "LFS files"]);
    env.await_notes(&src);
    assert!(env.git(&src, ["cat-file", "blob", "HEAD:big.bin"]).starts_with("version https://git-lfs.github.com/spec/v1"));
    // The object of gone.bin is not in the local store any more.
    let oid = env.git(&src, ["cat-file", "blob", "HEAD:gone.bin"]).lines().find_map(|l| l.strip_prefix("oid sha256:").map(String::from)).unwrap();
    std::fs::remove_file(src.join(".git/lfs/objects").join(&oid[..2]).join(&oid[2..4]).join(&oid)).unwrap();
    let from = src.to_str().unwrap();

    // A destination without LFS gets the content as an ordinary file.
    let plain = dest_repo(&env, "plain");
    let out = env.prov(&plain, ["cp", "--from", from, "big.bin", "big.bin"]).success();
    assert!(out.stdout.contains("big.bin  binary"), "{}", out.stdout);
    assert_eq!(std::fs::read_to_string(plain.join("big.bin")).unwrap(), content);
    env.git(&plain, ["commit", "-q", "-m", "Copy"]);
    assert_eq!(env.git(&plain, ["cat-file", "blob", "HEAD:big.bin"]) + "\n", content);
    assert_eq!(wait_checked(&plain), [] as [Value; 0], "the check agrees");

    // An object that is not in the source's store: refused, nothing copied.
    let missing = env.prov(&plain, ["cp", "--from", from, "gone.bin", "gone.bin"]).failure();
    assert!(missing.stderr.contains("gone.bin is a Git LFS pointer, and its content is not in the local LFS store"), "{}", missing.stderr);
    assert!(!plain.join("gone.bin").exists());

    // No git-lfs: refused, nothing copied. (Only where git-lfs is not in /usr/bin or /bin.)
    if !Path::new("/usr/bin/git-lfs").exists() && !Path::new("/bin/git-lfs").exists() {
        let bin = env.home().join("no-lfs-bin");
        std::fs::create_dir_all(&bin).unwrap();
        for tool in ["prov", "git-ai"] {
            let path = env.run(env.home(), "sh", ["-c", &format!("command -v {tool}")], &[], None).stdout.trim().to_string();
            std::os::unix::fs::symlink(path, bin.join(tool)).unwrap();
        }
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let none = env.run(&plain, "prov", ["cp", "--from", from, "big.bin", "again.bin"], &[("PATH", path.as_str())], None).failure();
        assert!(none.stderr.contains("git-lfs is not installed"), "{}", none.stderr);
        assert!(!plain.join("again.bin").exists());
    }

    // A destination that tracks the same pattern stores the content in its own LFS store.
    let tracked = dest_repo(&env, "tracked");
    let refused = env.try_git(&tracked, ["lfs", "install", "--local"]);
    assert!(!refused.ok && refused.stderr.contains("Hook already exists"), "{}", refused.stderr);
    env.git(&tracked, ["lfs", "install", "--local", "--force"]);
    env.prov(&tracked, ["init"]).success();
    let hooks = env.hooks_dir(&tracked);
    for name in ["post-commit", "post-merge"] {
        assert!(std::fs::read_to_string(hooks.join(name)).unwrap().contains("prov hook git"), "{name}");
        assert!(std::fs::read_to_string(hooks.join(format!("{name}.local"))).unwrap().contains("git lfs"), "{name}.local");
    }
    env.prov(&tracked, ["doctor"]).success();
    env.git(&tracked, ["lfs", "track", "*.bin"]);
    env.git(&tracked, ["add", ".gitattributes"]);
    env.git(&tracked, ["commit", "-q", "-m", "Track"]);
    env.prov(&tracked, ["cp", "--from", from, "big.bin", "big.bin"]).success();
    let begin = Instant::now();
    env.git(&tracked, ["commit", "-q", "-m", "Copy"]);
    assert!(begin.elapsed() < Duration::from_secs(10), "the commit waited {:?}", begin.elapsed());
    assert!(env.git(&tracked, ["cat-file", "blob", "HEAD:big.bin"]).starts_with("version https://git-lfs.github.com/spec/v1"));
    assert_eq!(std::fs::read_to_string(tracked.join("big.bin")).unwrap(), content);
    assert!(tracked.join(".git/lfs/objects").is_dir());
    assert_eq!(wait_checked(&tracked), [] as [Value; 0], "the check agrees");
}
