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

/// The counts of one target.
pub fn counts(short: impl Display, changed: impl Display, inserted: impl Display) -> String {
    format!("{short}: merged lines changed or deleted: {changed}. Lines inserted next to merged lines: {inserted}.")
}

pub fn other_count(other: impl Display) -> String {
    format!("Other lines changed or deleted: {other}.")
}

pub fn no_merged_line(short: impl Display) -> String {
    format!(
        "refused: the staged changes change or delete no line that came in with {short}, and insert no line next to \
         one. A revision must change the work that it names."
    )
}

pub const DRY_RUN: &str = "Dry run: nothing committed.";

/// The comment lines of the commit template. git removes them. One row for
/// each target: short ID, subject, merged lines changed or deleted, lines
/// inserted next to merged lines.
pub fn template_comment(targets: &[(String, String, usize, usize)], other: impl Display) -> String {
    let mut lines: Vec<String> = targets
        .iter()
        .map(|(short, subject, changed, inserted)| {
            format!(
                "prov revise: this commit revises the integration {short} \"{subject}\".\n\
                 Merged lines changed or deleted: {changed}. Lines inserted next to merged lines: {inserted}."
            )
        })
        .collect();
    lines.push(format!(
        "Other lines changed or deleted: {other}.\n\
         Write the subject on the first line. Keep the Prov-Revises trailers.\n\
         An unchanged message stops the commit."
    ));
    lines.join("\n")
}
