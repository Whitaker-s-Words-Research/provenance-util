//! The merge gate, in `prepare-commit-msg` of an integration. `prov check`
//! shows the same items.

use std::fmt::Display;

/// An item that needs a human approval: a protected path.
pub fn item_protected(path: impl Display) -> String {
    format!("{path}: protected path changed")
}

pub fn item_merge_resolution(path: impl Display, commit: impl Display) -> String {
    format!("{path}: changed by the merge commit {commit} on the incoming branch (a conflict resolution?)")
}

pub fn item_no_ranges(path: impl Display, commit: impl Display) -> String {
    format!(
        "{path}: commit {commit} adds lines, and git-ai lists none of them (a path that git-ai ignores, for \
         example vendor/ or a lock file?)"
    )
}

pub fn item_tainted(path: impl Display, start: impl Display, end: impl Display, commit: impl Display) -> String {
    format!(
        "{path}: lines {start}-{end} of commit {commit} changed while the occupant of the agent worktree changed (a session \
         that crashed or ended with no Stop, or two sessions at once). prov does not know which session wrote them."
    )
}

pub fn item_unknown_session(path: impl Display, start: impl Display, end: impl Display, commit: impl Display, session: impl Display) -> String {
    format!(
        "{path}: lines {start}-{end} of commit {commit} belong to the session {session}, which has no Prov-Session trailer and \
         no record (lines with no checkpoint, in a commit that skipped the hooks?)"
    )
}

pub fn item_no_note(path: impl Display, start: impl Display, end: impl Display, commit: impl Display) -> String {
    format!(
        "{path}: lines {start}-{end} of commit {commit} have no author: the commit has no git-ai note (was the git-ai \
         service running at the commit?)"
    )
}

pub fn item_human(path: impl Display, start: impl Display, end: impl Display, commit: impl Display) -> String {
    format!("{path}: lines {start}-{end} of commit {commit} are attributed to a human, in an agent commit")
}

pub fn item_untracked(path: impl Display, start: impl Display, end: impl Display, commit: impl Display) -> String {
    format!("{path}: lines {start}-{end} of commit {commit} have no author (untracked)")
}

pub fn late_note(commit: impl Display, delay: impl Display) -> String {
    format!("note: the git-ai note of commit {commit} came {delay} after `git-ai await` returned")
}

pub fn session_no_record(session: impl Display) -> String {
    format!("session {session} has no record in this clone. A session is integrated from the clone that ran it.")
}

pub fn session_open(session: impl Display, id: impl Display) -> String {
    format!("session {session} is open. Archive it first: prov session archive {id}")
}

pub fn session_closing(session: impl Display) -> String {
    format!("session {session} is closing. Wait for the archive step, then try again.")
}

pub fn session_failed(session: impl Display, error: impl Display, id: impl Display) -> String {
    format!("session {session} is failed ({error}). Run the archive step again: prov session archive {id}")
}

/// A Failed session with no error text.
pub const NO_ERROR_TEXT: &str = "no error text";

pub fn session_no_hash(session: impl Display) -> String {
    format!("session {session} has no transcript hash.")
}

pub fn session_open_in_range(session: impl Display, worktree: impl Display, branch: impl Display, id: impl Display) -> String {
    format!("session {session} is open in the worktree {worktree} on the branch {branch}. Archive it first: prov session archive {id}")
}

pub const ARCHIVED_NO_HASH: &str = "an archived session has no transcript hash";

pub const OCTOPUS: &str =
    "an octopus merge that contains agent commits is refused. Merge one agent branch at a time.\nTo stop: git merge --abort";

pub const NO_CONFIG: &str =
    "HEAD has no .provenance.toml. Run `prov init` on this branch and commit the file.\nTo stop: git merge --abort";

pub fn version_fails(error: impl Display) -> String {
    format!("the integration fails: {error}.\nTo stop: git merge --abort")
}

/// `git-ai await` failed. Also in `prov check`.
pub fn await_failed(error: impl Display) -> String {
    format!("warning: {error}. prov checks the note of each commit instead.")
}

pub const REFUSED: &str = "the merge gate refuses this integration.";

pub fn problem(text: impl Display) -> String {
    format!("  session: {text}")
}

pub const FIX_SESSIONS: &str = "The merge is still in progress. Correct the sessions, then run: git commit";
pub const ITEMS_HEADER: &str = "  these items need the approval of a human:";

pub fn item(text: impl Display) -> String {
    format!("    {text}")
}

pub const NEXT_STEPS: &str = "The merge is still in progress. Next steps:";

pub fn step_approve(paths: impl Display) -> String {
    format!("  1. prov approve --merge {paths}")
}

pub const STEP_COMMIT: &str = "  2. git commit";
pub const TO_STOP: &str = "To stop instead: git merge --abort";
