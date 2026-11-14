//! `prov revise`: a human commit that changes the lines of an integration,
//! with a `Prov-Revises` trailer (R-REV-1).

// A panic in a test is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{branch, Env};

const SESSION: &str = "3d0c2b9a-7f3d-4c7e-9a61-0d2f6b8e4a12";

#[test]
fn revise_an_integration() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = env.worktree(&repo, "20261113-rev");
    let base = env.git(&repo, ["rev-parse", "HEAD"]);

    env.start_session(&wt, SESSION).success();
    env.agent_edit(&wt, "src/a.py", "line 1\nline 2\nline 3\nline 4\n", SESSION);
    env.hook(&wt, "Stop", SESSION).success();
    env.agent_commit(&wt, SESSION, "Add a").success();
    env.end_session(&wt, SESSION);

    // A merge with no conflict runs `post-merge`, which prints the hint.
    let merge = env.try_git(&repo, ["merge", &branch("20261113-rev")]).success();
    let integration = env.git(&repo, ["rev-parse", "HEAD"]);
    assert!(merge.stderr.contains("prov revise"), "{}", merge.stderr);

    // A human commit after the integration: the target is still found.
    env.plain_write(&repo, "other.txt", "other\n");
    env.try_git(&repo, ["add", "other.txt"]).success();
    env.try_git(&repo, ["commit", "-q", "-m", "Other"]).success();

    // Nothing staged.
    let out = env.prov(&repo, ["revise", "--dry-run"]).failure();
    assert!(out.stderr.contains("nothing is staged"), "{}", out.stderr);
    assert!(out.stdout.contains("1 commit(s) since it"), "{}", out.stdout);

    // Lines inserted after the last merged line, and before the first: they count.
    env.plain_write(&repo, "src/a.py", "line 1\nline 2\nline 3\nline 4\nline 5\n");
    env.try_git(&repo, ["add", "src/a.py"]).success();
    let out = env.prov(&repo, ["revise", "--dry-run"]).success();
    assert!(out.stdout.contains("merged lines changed or deleted: 0. Lines inserted next to merged lines: 1."), "{}", out.stdout);
    env.plain_write(&repo, "src/a.py", "line 0\nline 1\nline 2\nline 3\nline 4\n");
    env.try_git(&repo, ["add", "src/a.py"]).success();
    let out = env.prov(&repo, ["revise", "--dry-run"]).success();
    assert!(out.stdout.contains("Lines inserted next to merged lines: 1."), "{}", out.stdout);
    env.plain_write(&repo, "src/a.py", "line 1\nline 2\nline 3\nline 4\n");

    // A new file alone, and lines inserted next to lines that did not come
    // with the merge: refused.
    env.plain_write(&repo, "src/test_a.py", "test = 1\n");
    env.plain_write(&repo, "other.txt", "other\nmore\n");
    env.try_git(&repo, ["add", "-A"]).success();
    let out = env.prov(&repo, ["revise", "--dry-run"]).failure();
    assert!(out.stderr.contains("change or delete no line") && out.stderr.contains("insert no line next to one"), "{}", out.stderr);
    env.try_git(&repo, ["reset", "-q", "--hard"]).success();

    // Only a change to a line that did not come with the merge: refused.
    env.plain_write(&repo, "README.md", "# changed\n");
    env.try_git(&repo, ["add", "-A"]).success();
    let out = env.prov(&repo, ["revise", "--dry-run"]).failure();
    assert!(out.stderr.contains("change or delete no line"), "{}", out.stderr);
    assert!(out.stdout.contains("merged lines changed or deleted: 0. Lines inserted next to merged lines: 0."), "{}", out.stdout);
    assert!(out.stdout.contains("Other lines changed or deleted: 1."), "{}", out.stdout);

    // A change to a merged line and a deleted merged line.
    env.plain_write(&repo, "src/a.py", "line 1\nline 2 revised\nline 4\n");
    env.try_git(&repo, ["add", "src/a.py"]).success();
    let out = env.prov(&repo, ["revise", "--dry-run"]).success();
    assert!(out.stdout.contains("merged lines changed or deleted: 2. Lines inserted next to merged lines: 0."), "{}", out.stdout);
    assert!(out.stdout.contains("Other lines changed or deleted: 1."), "{}", out.stdout);

    // A file with changes that are not staged: refused.
    env.plain_write(&repo, "src/a.py", "line 1\nline 2 revised again\nline 4\n");
    let out = env.prov(&repo, ["revise", "--dry-run"]).failure();
    assert!(out.stderr.contains("not staged: src/a.py"), "{}", out.stderr);
    env.plain_write(&repo, "src/a.py", "line 1\nline 2 revised\nline 4\n");

    // `--of` must name an integration that is an ancestor of HEAD.
    let out = env.prov(&repo, ["revise", "--dry-run", "--of", &base]).failure();
    assert!(out.stderr.contains("is not an integration"), "{}", out.stderr);
    env.prov(&repo, ["revise", "--dry-run", "--of", &integration]).success();

    // An agent cannot revise.
    let out = env.run(&repo, "prov", ["revise", "--dry-run"], &[("CLAUDECODE", "1")], None).failure();
    assert!(out.stderr.contains("only a human can revise"), "{}", out.stderr);

    // An unchanged message stops the commit.
    let head = env.git(&repo, ["rev-parse", "HEAD"]);
    env.prov(&repo, ["revise"]).failure();
    assert_eq!(env.git(&repo, ["rev-parse", "HEAD"]), head);

    // The human writes the subject.
    env.run(&repo, "prov", ["revise"], &[("GIT_EDITOR", "sed -i 1s/^/Fix\\ a/")], None).success();
    assert_eq!(env.git(&repo, ["log", "-1", "--format=%s"]), "Fix a");
    assert_eq!(env.trailers(&repo, "HEAD"), [format!("Prov-Revises: {integration}")]);
    assert!(!env.git(&repo, ["log", "-1", "--format=%B"]).contains("prov revise:"), "comment lines are removed");

    // A second revision of lines that the first one changed: they count,
    // because the first revision names the same target. A third one too.
    env.plain_write(&repo, "src/a.py", "line 1\nline 2 revised twice\nline 4\n");
    env.try_git(&repo, ["add", "src/a.py"]).success();
    let out = env.run(&repo, "prov", ["revise"], &[("GIT_EDITOR", "sed -i 1s/^/Fix\\ a\\ again/")], None).success();
    assert!(out.stdout.contains("merged lines changed or deleted: 1."), "{}", out.stdout);
    env.plain_write(&repo, "src/a.py", "line 1\nline 2 revised three times\nline 4\n");
    env.try_git(&repo, ["add", "src/a.py"]).success();
    let out = env.prov(&repo, ["revise", "--dry-run"]).success();
    assert!(out.stdout.contains("merged lines changed or deleted: 1."), "{}", out.stdout);
    env.try_git(&repo, ["reset", "-q", "--hard"]).success();

    // A revision is a human commit: prov check accepts it.
    env.await_notes(&repo);
    let check = env.prov(&repo, ["check", &format!("{base}..HEAD")]).success();
    assert!(check.stdout.contains("4 commit(s) checked, 0 failure(s)"), "{}", check.stdout);
}

/// A revision that crosses the work of two integrations names both. Each
/// target needs a counted line of its own.
#[test]
fn revise_two_integrations() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let mut merges = Vec::new();
    for (name, session, file) in [("rev-a", "sess-ra", "a.py"), ("rev-b", "sess-rb", "b.py")] {
        let wt = env.worktree(&repo, name);
        env.start_session(&wt, session).success();
        env.agent_edit(&wt, file, "one\ntwo\n", session);
        env.agent_commit(&wt, session, &format!("Add {file}")).success();
        env.end_session(&wt, session);
        env.try_git(&repo, ["merge", &branch(name)]).success();
        merges.push(env.git(&repo, ["rev-parse", "HEAD"]));
    }
    let (first, second) = (&merges[0], &merges[1]);

    // Only lines of the first: a revision of both is refused, and names the second.
    env.plain_write(&repo, "a.py", "one\ntwo fixed\n");
    env.try_git(&repo, ["add", "-A"]).success();
    let out = env.prov(&repo, ["revise", "--dry-run", "--of", first, "--of", second]).failure();
    assert!(out.stderr.contains(&format!("came in with {}", &second[..7])), "{}", out.stderr);

    // Lines of both.
    env.plain_write(&repo, "b.py", "one\ntwo\nthree\n");
    env.try_git(&repo, ["add", "-A"]).success();
    env.run(&repo, "prov", ["revise", "--of", first, "--of", second], &[("GIT_EDITOR", "sed -i 1s/^/Fix\\ both/")], None).success();
    assert_eq!(env.trailers(&repo, "HEAD"), [format!("Prov-Revises: {first}"), format!("Prov-Revises: {second}")]);

    // Without --of, the target is the latest integration only.
    env.plain_write(&repo, "b.py", "one\ntwo\nthree\nfour\n");
    env.try_git(&repo, ["add", "-A"]).success();
    let out = env.prov(&repo, ["revise", "--dry-run"]).success();
    assert!(out.stdout.contains(&format!("Revises {}", &second[..7])) && !out.stdout.contains(&first[..7]), "{}", out.stdout);
}
