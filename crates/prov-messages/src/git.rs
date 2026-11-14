//! Errors of git commands and of the git repository.

use std::fmt::{Debug, Display};

pub const PATH_NOT_UTF8: &str = "git printed a path that is not UTF-8";
pub const OUTPUT_NOT_UTF8: &str = "git output is not UTF-8";

pub fn failed(args: impl Debug, stderr: impl Display) -> String {
    format!("{args:?} failed: {stderr}")
}

pub const CANNOT_READ_MERGE_HEAD: &str = "cannot read MERGE_HEAD";

/// The context of an error in the config file of a commit.
pub fn at_rev(rev: impl Display) -> String {
    format!("at {rev}")
}

pub const UNEXPECTED_LOG: &str = "unexpected output of git log";

pub fn no_commit(rev: impl Display) -> String {
    format!("no commit {rev}")
}

pub fn not_in_worktree(path: impl Display, worktree: impl Display) -> String {
    format!("{path} is not in the worktree {worktree}")
}

pub fn not_utf8(path: impl Display) -> String {
    format!("{path} is not UTF-8")
}

pub fn worktree_root(path: impl Display) -> String {
    format!("{path} is the root of the worktree, not a file")
}
