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

    // Only an added line: refused.
    env.plain_write(&repo, "src/a.py", "line 1\nline 2\nline 3\nline 4\nline 5\n");
    env.try_git(&repo, ["add", "src/a.py"]).success();
    let out = env.prov(&repo, ["revise", "--dry-run"]).failure();
    assert!(out.stderr.contains("change or delete no line"), "{}", out.stderr);

    // Only a change to a line that did not come with the merge: refused.
    env.plain_write(&repo, "src/a.py", "line 1\nline 2\nline 3\nline 4\n");
    env.plain_write(&repo, "README.md", "# changed\n");
    env.try_git(&repo, ["add", "-A"]).success();
    let out = env.prov(&repo, ["revise", "--dry-run"]).failure();
    assert!(out.stderr.contains("change or delete no line"), "{}", out.stderr);
    assert!(out.stdout.contains("Merged lines changed or deleted: 0. Other lines changed or deleted: 1."), "{}", out.stdout);

    // A change to a merged line and a deleted merged line.
    env.plain_write(&repo, "src/a.py", "line 1\nline 2 revised\nline 4\n");
    env.try_git(&repo, ["add", "src/a.py"]).success();
    let out = env.prov(&repo, ["revise", "--dry-run"]).success();
    assert!(out.stdout.contains("Merged lines changed or deleted: 2. Other lines changed or deleted: 1."), "{}", out.stdout);

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

    // A second revision counts only lines that still come from the merge.
    env.plain_write(&repo, "src/a.py", "line 1\nline 2 revised twice\nline 4\n");
    env.try_git(&repo, ["add", "src/a.py"]).success();
    let out = env.prov(&repo, ["revise", "--dry-run"]).failure();
    assert!(out.stdout.contains("Merged lines changed or deleted: 0."), "{}", out.stdout);
    env.plain_write(&repo, "src/a.py", "line 1\nline 2 revised twice\nline 4 revised\n");
    env.try_git(&repo, ["add", "src/a.py"]).success();
    env.prov(&repo, ["revise", "--dry-run"]).success();
    env.try_git(&repo, ["reset", "-q", "--hard"]).success();

    // A revision is a human commit: prov check accepts it.
    env.await_notes(&repo);
    let check = env.prov(&repo, ["check", &format!("{base}..HEAD")]).success();
    assert!(check.stdout.contains("3 commit(s) checked, 0 failure(s)"), "{}", check.stdout);
}
