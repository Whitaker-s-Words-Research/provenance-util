//! `prov check`.

use std::fmt::Display;

pub const RANGE_SYNTAX: &str = "give a range as <base>..<tip>";
pub const INTO_WITH_RANGE: &str = "--into has no meaning with a range";

pub fn on_agent_branch(branch: impl Display) -> String {
    format!(
        "the branch `{branch}` is an agent branch, and an agent branch is not a merge target. \
         Name the human branch: prov check --into <branch>"
    )
}

pub const DETACHED: &str = "HEAD is detached. Name the human branch: prov check --into <branch>";

pub fn into_agent_branch(target: impl Display) -> String {
    format!("`{target}` is an agent branch. --into takes a human branch: a branch that does not match `agent/**`")
}

pub fn no_branch(target: impl Display) -> String {
    format!("no branch or commit `{target}`")
}

pub fn no_commit(rev: impl Display) -> String {
    format!("no commit `{rev}`")
}

// The report of an agent branch.

pub fn commits(count: impl Display) -> String {
    format!("  commits: {count}")
}

pub fn commit_line(text: impl Display) -> String {
    format!("    {text}")
}

pub fn session_state(session: impl Display, state: impl Display) -> String {
    format!("{session} ({state})")
}

pub fn sessions(list: impl Display) -> String {
    format!("  sessions: {list}")
}

/// The list of sessions is empty.
pub const NO_SESSIONS: &str = "none";

pub fn doubtful_lines(count: impl Display) -> String {
    format!("  doubtful lines: {count}")
}

pub fn protected_paths(paths: impl Display) -> String {
    format!("  protected paths changed: {paths}")
}

pub fn nothing_waits(into: impl Display) -> String {
    format!("No agent branch waits for integration into {into}.")
}

pub fn fail_commit(commit: impl Display, message: impl Display) -> String {
    format!("FAIL {commit} {message}")
}

pub fn fail(message: impl Display) -> String {
    format!("FAIL {message}")
}

pub fn summary(commits: impl Display, failures: impl Display) -> String {
    format!("{commits} commit(s) checked, {failures} failure(s).")
}

pub fn ok(into: impl Display) -> String {
    format!("OK: the merge gate has nothing to refuse for a merge into {into}.")
}

/// The state of a session that has no record in this clone. Also in the JSON output.
pub const NO_RECORD: &str = "no record";

// The failures of `prov check <agent branch>`.

pub fn config_of(branch: impl Display) -> String {
    format!("the config of `{branch}`")
}

pub fn branch_no_config(branch: impl Display) -> String {
    format!("the branch `{branch}` has no .provenance.toml")
}

pub fn needs_approval(item: impl Display) -> String {
    format!("needs approval at the merge: {item}")
}

// The failures of `prov check <base>..<tip>`. Each one follows the commit ID.

pub fn rev_no_config(rev: impl Display) -> String {
    format!("{rev} has no .provenance.toml")
}

pub const NOT_HUMAN_COMMIT: &str = "is not a human commit, on the first-parent line of a human branch (a fast-forward, rebase or \
     cherry-pick of agent commits?)";

pub const OCTOPUS: &str = "is an octopus merge that contains agent commits";
pub const PARENT_NO_CONFIG: &str = "brings in agent commits, but its first parent has no .provenance.toml";
pub const NO_VERSION_TRAILER: &str =
    "brings in agent commits and has no Prov-Version trailer: it did not pass the merge gate (a merge on a forge?)";

pub fn no_trailer_group(session: impl Display) -> String {
    format!("has no trailer group for the session {session}")
}

pub fn no_transcript_hash(session: impl Display) -> String {
    format!("has no valid Prov-Transcript-SHA256 for the session {session}")
}

pub fn no_approval(item: impl Display) -> String {
    format!("has no Prov-Approved trailer for {item}")
}

pub fn ai_lines_in_human_commit(path: impl Display, session: impl Display) -> String {
    format!(
        "is a human commit, but its note gives lines of {path} to the AI session {session} (a squash or cherry-pick \
         of agent commits, or a session in a human worktree?)"
    )
}
