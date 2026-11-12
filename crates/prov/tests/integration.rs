//! The "done when" test of the core: a recorded Claude Code session in an
//! agent worktree, the merge gate, an approval written by hand, and
//! `prov check` on the result.

mod common;

use common::{branch, Env, FIXTURE_MODEL};

const SESSION: &str = "5c1e2b9a-7f3d-4c7e-9a61-0d2f6b8e4a10";

#[test]
fn session_merge_gate_and_check() {
    let Some(env) = Env::new() else { return };
    let repo = env.repo("project");
    let wt = env.worktree(&repo, "20261111-parser");
    let base = env.git(&repo, ["rev-parse", "HEAD"]);

    // --- The session ----------------------------------------------------
    let start = env.start_session(&wt, SESSION).success();
    assert_eq!(start.stdout, "", "a session in a free agent worktree gets no message");
    let record = env.record(&repo, SESSION).unwrap();
    assert_eq!(record["state"], "open");
    assert_eq!(record["agent"], "claude");
    assert_eq!(record["branch"], branch("20261111-parser"));
    assert_eq!(record["worktree"], wt.to_str().unwrap());
    assert_eq!(record["model"], "claude-opus-5-5", "the model of the SessionStart payload");
    assert_eq!(record["agent_pid"], std::process::id());

    let prompt = env.hook(&wt, "UserPromptSubmit", SESSION).success();
    assert_eq!(prompt.stdout, "", "output at UserPromptSubmit would enter the agent's context");

    // One file through the agent's edit tool, one file from a process that
    // the agent started (no checkpoint), and one file that a human edited.
    env.agent_edit(&wt, "src/parse.py", "def parse(text):\n    return text.split()\n", SESSION);
    env.plain_write(&wt, "generated/table.txt", "row 1\nrow 2\nrow 3\n");
    env.human_edit(&wt, "notes.txt", "a note from the human\n");

    env.hook(&wt, "Stop", SESSION).success();
    assert_eq!(env.record(&repo, SESSION).unwrap()["model"], FIXTURE_MODEL, "the model of the latest assistant message");

    env.agent_commit(&wt, SESSION, "Add the parser").success();
    assert_eq!(env.trailers(&wt, "HEAD"), [format!("Prov-Session: claude:{SESSION}")]);
    let agent_commit = env.git(&wt, ["rev-parse", "HEAD"]);

    // The sweep gave the plain file to the session, with the right model.
    let ai = |path: &str| (path.to_string(), "claude".to_string(), SESSION.to_string(), FIXTURE_MODEL.to_string());
    assert_eq!(
        env.authorship(&wt, &agent_commit),
        [ai("generated/table.txt"), ("notes.txt".into(), "human".into(), String::new(), String::new()), ai("src/parse.py")]
    );

    // --- The archive ----------------------------------------------------
    let record = env.end_session(&wt, SESSION);
    let hash = record["archive_sha256"].as_str().unwrap().to_string();
    let archive = std::path::PathBuf::from(record["archive_file"].as_str().unwrap());
    assert_eq!(archive, repo.join("llm-logs/claude").join(format!("{SESSION}-{}.tar.gz", &hash[..12])));
    assert_eq!(common::sha256_of_gunzipped(&archive), hash, "the hash is of the tar bytes, before compression");
    assert_eq!(common::tar_names(&archive), [format!("{SESSION}/subagents/agent-a1.jsonl"), "transcript.jsonl".to_string()]);

    // --- prov check <agent branch>: what the gate will show ---------------
    let check = env.prov(&repo, ["check", &branch("20261111-parser")]).failure();
    assert!(check.stdout.contains("notes.txt: lines 1-1"), "{}", check.stdout);
    assert!(!check.stdout.contains("table.txt") && !check.stdout.contains("parse.py"), "{}", check.stdout);

    // --- The merge gate ---------------------------------------------------
    let merge = env.try_git(&repo, ["merge", &branch("20261111-parser")]).failure();
    assert!(merge.stderr.contains("the merge gate refuses this integration"), "{}", merge.stderr);
    assert!(merge.stderr.contains("notes.txt: lines 1-1"), "{}", merge.stderr);
    assert!(merge.stderr.contains("prov approve --merge notes.txt"), "{}", merge.stderr);
    // git leaves the merge in progress, with the changes staged.
    assert_eq!(env.git(&repo, ["rev-parse", "MERGE_HEAD"]), agent_commit);
    assert!(env.git(&repo, ["diff", "--cached", "--name-only"]).contains("notes.txt"));
    // `--no-verify` does not skip the gate.
    env.try_git(&repo, ["commit", "--no-verify", "--no-edit"]).failure();
    assert_eq!(env.git(&repo, ["rev-parse", "HEAD"]), base);

    // The human approves by writing the trailer into the merge message.
    let merge_msg = repo.join(".git/MERGE_MSG");
    let mut message = std::fs::read_to_string(&merge_msg).unwrap();
    message.push_str("\nProv-Approved: notes.txt ack\n");
    std::fs::write(&merge_msg, message).unwrap();
    env.try_git(&repo, ["commit", "--no-edit"]).success();

    // --- The integration --------------------------------------------------
    let parents = env.git(&repo, ["log", "-1", "--format=%P"]);
    assert_eq!(parents, format!("{base} {agent_commit}"));
    let version = env.prov(&repo, ["--version"]).success().stdout.trim().replace("prov ", "");
    assert_eq!(
        env.trailers(&repo, "HEAD"),
        [
            "Prov-Approved: notes.txt ack".to_string(),
            format!("Prov-Session: claude:{SESSION}"),
            format!("Prov-Transcript-SHA256: {hash}"),
            format!("Prov-Version: {version}"),
        ]
    );
    // The integration deletes the records of its sessions. The archive stays.
    assert!(env.record(&repo, SESSION).is_none());
    assert!(archive.is_file());

    // --- prov check <base>..<tip> ------------------------------------------
    env.await_notes(&repo);
    let check = env.prov(&repo, ["check", &format!("{base}..HEAD")]).success();
    assert!(check.stdout.contains("1 commit(s) checked, 0 failure(s)"), "{}", check.stdout);
    let json = env.prov(&repo, ["check", &format!("{base}..HEAD"), "--json"]).success();
    let json: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
    assert_eq!(json["ok"], true);

    // Nothing waits any more.
    let list = env.prov(&repo, ["check"]).success();
    assert!(list.stdout.contains("No agent branch waits"), "{}", list.stdout);
}
