//! `prov discard`.

use std::fmt::Display;

pub fn agent_env(var: impl Display) -> String {
    format!("refused: only a human can run `prov discard`. {var} is set: an agent started this command.")
}

pub const AGENT_WORKTREE: &str =
    "refused: `prov discard` does not run in an agent worktree. Run it from a human worktree.";

pub fn refs_kept_changing(attempts: impl Display) -> String {
    format!("refused: refs of this repository changed {attempts} times during the check. Run it again when they stop changing.")
}

pub const REF_CHANGED: &str = "a ref changed during the check. Checking again.";

pub const STOP_PROCESSES: &str = "Stop them: the worktree is removed now.";

pub fn discarded(branch: impl Display) -> String {
    format!("Discarded the branch {branch}.")
}

pub fn remote_stays(remote: impl Display, name: impl Display, branch: impl Display) -> String {
    format!("The remote branch {remote} stays. To delete it: git push {name} --delete {branch}")
}

pub fn session_not_agent_branch(id: impl Display, branch: impl Display) -> String {
    format!("session {id} is on {branch}, which is not an agent branch")
}

pub fn not_found(target: impl Display) -> String {
    format!("`{target}` is not an agent branch, and not a session. See: git branch --list 'agent/*', prov session list")
}

pub fn checked_out_here(branch: impl Display) -> String {
    format!("refused: the branch {branch} is checked out here. Run `prov discard` from another worktree.")
}

pub fn checked_out_in_main(branch: impl Display, worktree: impl Display) -> String {
    format!("refused: the branch {branch} is checked out in the main worktree {worktree}. Check out another branch there first.")
}

pub fn session_of_other_branch(commit: impl Display, session: impl Display, other: impl Display, branch: impl Display, id: impl Display) -> String {
    format!(
        "refused: commit {commit} names session {session}, and its record is for the branch {other}, not {branch}. \
         Discard that branch, or drop the record (prov session drop {id})."
    )
}

pub fn busy(branch: impl Display, sessions: impl Display) -> String {
    format!("refused: a session of {branch} is open or closing: {sessions}. Archive it first: prov session archive <id>")
}

pub fn busy_session(id: impl Display, state: impl Display) -> String {
    format!("{id} ({state})")
}

pub fn named_on_other_ref(commit: impl Display, session: impl Display, branch: impl Display) -> String {
    format!(
        "refused: commit {commit} on another ref names session {session}. That ref needs the session's record or archive. \
         To remove only the worktree: prov finish {branch}"
    )
}

pub fn copied_from_own(commit: impl Display, source: impl Display, branch: impl Display) -> String {
    format!(
        "refused: commit {commit} copied a file from commit {source} of {branch} (Copied-From). \
         The transcripts of that copy are found through this branch."
    )
}

pub fn copy_in_progress(id: impl Display, source: impl Display, branch: impl Display) -> String {
    format!("refused: the copy {id} has its source in commit {source} of {branch}. Commit or delete the copied files first.")
}

pub fn merge_in_progress(branch: impl Display, worktree: impl Display) -> String {
    format!("refused: a merge of {branch} is in progress in {worktree}. Finish it or run `git merge --abort` there first.")
}

pub fn session_became(id: impl Display, state: impl Display) -> String {
    format!("refused: session {id} became {state} during the check. Nothing was deleted.")
}

pub fn session_moved(id: impl Display, branch: impl Display) -> String {
    format!("refused: session {id} now has a record for the branch {branch}. Nothing was deleted.")
}

pub fn json_error(error: impl Display) -> String {
    format!("error: {error}")
}

pub const WILL_DELETE: &str = "Will delete:";
pub const DELETING: &str = "Deleting:";

pub fn item_worktree(worktree: impl Display, changed: impl Display) -> String {
    format!("  the worktree {worktree}, with {changed} changed files that are not committed")
}

pub fn item_branch(branch: impl Display, own: impl Display) -> String {
    format!("  the branch {branch}, with {own} commits that no other ref has")
}

pub fn item_record(agent: impl Display, id: impl Display, state: impl Display) -> String {
    format!("  the record of session {agent}:{id} ({state})")
}

pub fn item_session(agent: impl Display, id: impl Display) -> String {
    format!("  session {agent}:{id} (no record)")
}

pub fn item_archive(archive: impl Display) -> String {
    format!("  the archive {archive}")
}

pub const KEEPS: &str = "It keeps the agent's own transcripts and the git-ai notes.";

pub fn keeps_remote(remote: impl Display) -> String {
    format!("It keeps the remote branch {remote}.")
}

pub const DRY_RUN: &str = "Dry run: nothing was deleted.";

pub fn not_deleted(branch: impl Display) -> String {
    format!("Nothing was deleted. To delete it: prov discard {branch} --yes")
}
