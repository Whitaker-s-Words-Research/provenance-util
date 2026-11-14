//! The warning about processes that still run in a worktree (R-SWEEP-4).

use std::fmt::Display;

pub fn busy_header(worktree: impl Display) -> String {
    format!("warning: processes still run in the worktree {worktree}:")
}

/// One process of the list.
pub fn busy_process(pid: impl Display, command: impl Display) -> String {
    format!("  {pid} {command}")
}

/// After the list. The caller adds what happens next.
pub const BUSY_RISK: &str = "If one of them writes files there (a development server, a watcher, a build), the next session in this \
     worktree takes those changes as its own work. ";
