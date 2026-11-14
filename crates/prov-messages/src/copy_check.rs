//! The copy guard of `pre-commit`, and the deferred check of a copy
//! (`prov copy-check`).

use std::fmt::Display;

pub fn changed_copy(dest: impl Display) -> String {
    format!(
        "refused: {dest} is a file of `prov cp`, and it is different from the copied bytes. Commit the copy \
         unchanged; change the file in a later commit.\nTo give up the copy: git rm -f --cached -- {dest} (and \
         delete the file), then copy it again if necessary."
    )
}

pub fn not_started(error: impl Display, id: impl Display) -> String {
    format!("warning: the copy check did not start: {error}. Run it: prov copy-check {id}")
}

pub const CANNOT_SPAWN: &str = "cannot start the copy check";

/// The author of a line, in a detail of the check and in `prov cp --plan`.
pub const AUTHOR_HUMAN: &str = "human";

pub fn author_ai(tool: impl Display, model: impl Display, session: impl Display) -> String {
    format!("ai {tool}/{model} ({session})")
}

/// A line that git-ai lists with no author.
pub const AUTHOR_NONE: &str = "no author";

/// A line of a plan whose author is not decided yet.
pub const AUTHOR_UNRESOLVED: &str = "?";

pub fn line_counts(plan: impl Display, file: impl Display) -> String {
    format!("the plan has {plan} lines, and the committed file has {file}")
}

pub fn no_group(line: impl Display) -> String {
    format!("line {line}: no group")
}

pub fn differs(line: impl Display, plan: impl Display, note: impl Display, count: impl Display) -> String {
    format!("line {line}: the plan has `{plan}`, and the note has `{note}` ({count} line(s) differ)")
}

pub fn moved(count: impl Display) -> String {
    format!("the authors of {count} line(s) moved between identical lines")
}

pub fn not_a_worktree(path: impl Display) -> String {
    format!("{path} is not a worktree")
}

pub fn different_bytes(commit: impl Display) -> String {
    format!("the file in the commit {commit} is different from the copied bytes")
}

pub fn no_note(commit: impl Display, seconds: impl Display) -> String {
    format!("the commit {commit} has no git-ai note after {seconds} seconds")
}

pub const TOO_MANY_LINES: &str = "the file has too many lines";

pub fn ignored(dest: impl Display, commit: impl Display) -> String {
    format!(
        "git-ai lists no line of {dest} in the commit {commit}. git-ai ignores some paths (for example \
         `**/vendor/**` and lock files) and records no authorship for them"
    )
}
