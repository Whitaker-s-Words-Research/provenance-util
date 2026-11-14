//! `prov finish`.

use std::fmt::Display;

pub fn unknown_target(target: impl Display, error: impl Display) -> String {
    format!("`{target}` is not a branch with a worktree, and not a session: {error}")
}

pub fn worktree_missing(worktree: impl Display, session: impl Display) -> String {
    format!("the worktree {worktree} of session {session} does not exist")
}

pub fn main_worktree(worktree: impl Display) -> String {
    format!("{worktree} is the main worktree. `prov finish` removes only a linked worktree.")
}

pub fn runs_inside(worktree: impl Display) -> String {
    format!("this command runs inside {worktree}. Run it from another worktree.")
}

/// One session of the list in [`busy`].
pub fn session_state(id: impl Display, state: impl Display) -> String {
    format!("{id} ({state})")
}

pub fn busy(worktree: impl Display, sessions: impl Display) -> String {
    format!(
        "refused: a session in {worktree} is open or closing: {sessions}. End the session or run `prov session close <id>`. \
         `--override` closes each open session first."
    )
}

pub fn not_archived(id: impl Display, reason: impl Display) -> String {
    format!("session {id} did not archive ({reason}). The worktree stays.")
}

/// After the list of processes that still run in the worktree.
pub const STOP_PROCESSES: &str = "Stop them: the worktree is removed now.";

pub fn will_close(id: impl Display) -> String {
    format!("will close session {id}")
}

pub fn closed(id: impl Display) -> String {
    format!("closed session {id}")
}

/// Shown for a worktree with no branch checked out.
pub const DETACHED: &str = "(detached)";

pub fn will_remove(worktree: impl Display, branch: impl Display) -> String {
    format!("Dry run: will remove the worktree {worktree}. The branch {branch} stays.")
}

pub fn removed(worktree: impl Display, branch: impl Display) -> String {
    format!("Removed the worktree {worktree}. The branch {branch} stays.")
}

pub fn merged(here: impl Display, command: impl Display) -> String {
    format!("It is merged into {here}. To delete it: {command}")
}

pub fn not_merged(here: impl Display, command: impl Display) -> String {
    format!("It is not merged into {here}. To delete it anyway, with its commits: {command}")
}
