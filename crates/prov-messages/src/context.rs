//! Shared by the commands of `prov`.

use std::fmt::Display;

pub const NOT_IN_WORKTREE: &str = "not in a git worktree";

/// A warning: the error of a step that does not stop the command.
pub fn warning(error: impl Display) -> String {
    format!("warning: {error}")
}
