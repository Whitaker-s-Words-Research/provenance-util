//! The live state: session, worktree, approval and copy records.

use std::fmt::Display;

pub fn already_archived(id: impl Display) -> String {
    format!("session {id} is already archived")
}

/// The error of a Closing session whose archive step did not finish.
pub const ARCHIVE_TIMED_OUT: &str = "the archive step did not finish in 5 minutes";

pub const CANNOT_READ_APPROVALS: &str = "cannot read the approval records";
pub const CANNOT_READ_WORKTREES: &str = "cannot read the worktree records";
pub const CANNOT_READ_COPIES: &str = "cannot read the copy records";
pub const CANNOT_READ_SESSIONS: &str = "cannot read the session records";
pub const CANNOT_DELETE_WORKTREE: &str = "cannot delete the worktree record";
pub const NO_DIRECTORY: &str = "no directory";

pub fn invalid_session_id(id: impl Display) -> String {
    format!("invalid session ID `{id}`")
}
