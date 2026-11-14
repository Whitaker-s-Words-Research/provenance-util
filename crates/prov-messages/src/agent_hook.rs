//! `prov hook <adapter> <event>`: the messages of an agent hook. Each one
//! starts with `prov: `, because it can reach the agent's context.

use std::fmt::Display;

pub fn unknown_adapter(name: impl Display) -> String {
    format!("unknown adapter `{name}`")
}

/// Shown when no branch is checked out.
pub const DETACHED_HEAD: &str = "detached HEAD";

/// At the start of a session outside an agent worktree.
pub fn not_agent_worktree(branch: impl Display, agent: impl Display) -> String {
    format!(
        "prov: this session is not in an agent worktree (branch: {branch}). prov records no session here, \
         and it refuses a commit by an agent on this branch. Agent work reaches this branch only \
         through a merge of an agent branch. Stop, and ask the human to start the session with \
         `prov start {agent}` or in a worktree on a branch that matches `agent/**`."
    )
}

/// The version of prov does not satisfy `required_version`.
pub fn version_warning(error: impl Display) -> String {
    format!("prov: warning: {error}.")
}

pub fn warning(error: impl Display) -> String {
    format!("prov: warning: {error}")
}

pub fn sweep_failed(error: impl Display) -> String {
    format!("prov: warning: the sweep failed: {error}")
}

/// The error of a session whose archive step did not start.
pub fn cannot_start_archive(error: impl Display) -> String {
    format!("cannot start the archive step: {error}")
}

/// At the start of a second open session in one agent worktree.
pub fn contended(others: impl Display, agent: impl Display) -> String {
    format!(
        "prov: STOP. This agent worktree already has an open session ({others}). An agent worktree holds one \
         open session at a time. While more than one is open, prov runs no sweep here and refuses every \
         commit. Change no files. Tell the human to end the other session or to run \
         `prov session archive <id>`, or to start this session in its own worktree with `prov start {agent}`."
    )
}

pub const GITAI_UNAVAILABLE: &str = "prov: warning: the git-ai service is not available. prov started it. While it is not available, \
     prov refuses commits in this worktree. Check with `git-ai bg status`.";
