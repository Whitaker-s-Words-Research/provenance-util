//! `prov revise`.

use std::fmt::Display;

pub fn agent_env(var: impl Display) -> String {
    format!("refused: only a human can revise. {var} is set: an agent started this command.")
}

pub const AGENT_WORKTREE: &str =
    "refused: only a human can revise, and not in an agent worktree. Revise on a human branch, after the merge.";

pub fn no_commit(rev: impl Display) -> String {
    format!("`{rev}` is not a commit")
}

pub fn not_integration(rev: impl Display) -> String {
    format!("`{rev}` is not an integration: a merge with a `Prov-Session` trailer")
}

pub fn not_ancestor(rev: impl Display) -> String {
    format!("`{rev}` is not an ancestor of HEAD")
}

pub const NO_INTEGRATION: &str =
    "no integration on the first-parent line from HEAD. Name one with --of <merge>.";

/// The target, and the number of commits since it.
pub fn target(short: impl Display, subject: impl Display, since: impl Display) -> String {
    format!("Revises {short} \"{subject}\". {since} commit(s) since it.")
}

pub const NOTHING_STAGED: &str = "nothing is staged. Stage your changes to the merged lines first: git add <path>";

pub fn partly_staged(paths: impl Display) -> String {
    format!(
        "these files also have changes that are not staged: {paths}. git-ai records the file in the working tree. \
         Stage the changes, or set them aside."
    )
}

pub const UNEXPECTED_DIFF: &str = "unexpected output of git diff";

pub fn counts(merged: impl Display, other: impl Display) -> String {
    format!("Merged lines changed or deleted: {merged}. Other lines changed or deleted: {other}.")
}

pub const NO_MERGED_LINE: &str = "refused: the staged changes change or delete no line that came in with this merge. \
     A revision must change the work that it names. Lines that are only added do not count.";

pub const DRY_RUN: &str = "Dry run: nothing committed.";

/// The comment lines of the commit template. git removes them.
pub fn template_comment(short: impl Display, subject: impl Display, merged: impl Display, other: impl Display) -> String {
    format!(
        "prov revise: this commit revises the integration {short} \"{subject}\".\n\
         Merged lines changed or deleted: {merged}. Other lines changed or deleted: {other}.\n\
         Write the subject on the first line. Keep the Prov-Revises trailer.\n\
         An unchanged message stops the commit."
    )
}
