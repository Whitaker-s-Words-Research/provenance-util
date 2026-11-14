//! `prov start`.

use std::fmt::Display;

pub const CANNOT_READ_URANDOM: &str = "cannot read /dev/urandom";
pub const BASE_NO_NAME: &str = "the base directory has no name";
pub const BASE_NO_PARENT: &str = "the base directory has no parent";

pub fn agent_not_configured(agent: impl Display, agents: impl Display) -> String {
    format!("the agent `{agent}` is not in `agents` of .provenance.toml ({agents})")
}

pub const NAME_CHARACTERS: &str = "--name takes letters, digits and `-` only";

pub fn not_agent_branch(branch: impl Display) -> String {
    format!("the branch `{branch}` does not match `agent/**`")
}

pub fn branch_exists(branch: impl Display) -> String {
    format!("the branch `{branch}` exists. Give another --name, or continue its session with --resume.")
}

pub fn exists(path: impl Display) -> String {
    format!("{path} exists")
}

/// On standard error, after the path of the worktree.
pub fn worktree(dry_run: bool, created: bool, branch: impl Display, from: Option<impl Display>) -> String {
    let from = from.map_or_else(String::new, |f| format!(", from {f}"));
    match (dry_run, created) {
        (false, true) => format!("prov: worktree created on the branch {branch}{from}"),
        (false, false) => format!("prov: worktree exists on the branch {branch}{from}"),
        (true, true) => format!("prov: dry run: worktree created on the branch {branch}{from}"),
        (true, false) => format!("prov: dry run: worktree exists on the branch {branch}{from}"),
    }
}

/// After the list of processes that still run in the worktree.
pub const STOP_PROCESSES: &str = "Stop them before the session goes on, or its sweep takes their changes.";

pub const NO_IDENTITY: &str = "the agent has no commit identity";

pub fn cannot_start(command: impl Display) -> String {
    format!("cannot start `{command}`")
}

// `prov start --resume`

pub fn session_on_other_branch(session: impl Display, recorded: impl Display, branch: impl Display) -> String {
    format!("the session {session} is on the branch `{recorded}`, not `{branch}`")
}

pub fn session_no_record(session: impl Display) -> String {
    format!("the session {session} has no record (it is integrated, or it ran in another clone). Also give --branch.")
}

pub fn no_branch(branch: impl Display) -> String {
    format!("no branch `{branch}`")
}

pub fn other_checkout(worktree: impl Display, branch: impl Display) -> String {
    format!("{worktree} exists, and it does not have the branch `{branch}` checked out")
}
