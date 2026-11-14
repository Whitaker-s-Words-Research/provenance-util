//! `prov hook git <name>`: the git hooks.

use std::fmt::Display;

pub fn unknown_hook(name: impl Display) -> String {
    format!("unknown git hook `{name}`")
}

pub fn cannot_run(path: impl Display) -> String {
    format!("cannot run {path}")
}

pub fn local_failed(path: impl Display) -> String {
    format!("the hook {path} failed")
}

pub fn approval_cancelled(path: impl Display, choice: impl Display) -> String {
    format!("warning: {path} changed after `prov approve --{choice}`. The approval is cancelled; approve the file again.")
}

pub fn config_fallback(error: impl Display) -> String {
    format!("warning: {error}. Using the config of HEAD.")
}

/// The version of prov does not satisfy `required_version`.
pub fn version_warning(error: impl Display) -> String {
    format!("warning: {error}.")
}

/// Why prov takes a commit for an agent's commit: an environment variable.
pub fn agent_env(var: impl Display) -> String {
    format!("{var} is set")
}

/// Why prov takes a commit for an agent's commit: the author email.
pub fn agent_email(email: impl Display) -> String {
    format!("the author email is {email}")
}

/// Shown when no branch is checked out.
pub const DETACHED_HEAD: &str = "detached HEAD";

pub fn agent_commit_on_human_branch(why: impl Display, branch: impl Display) -> String {
    format!(
        "refused: a commit by an agent ({why}) on the branch `{branch}`. An agent commits only on a branch \
         that matches `agent/**`. Agent work reaches this branch only through a merge of an agent branch."
    )
}

/// A human commit while the git-ai service is not available.
pub const GITAI_UNAVAILABLE_HUMAN: &str = "refused: the git-ai service is not available, so this commit would get no git-ai note \
     (and an amend would leave the note on the old commit). Start it with `git-ai bg start`, \
     check it with `git-ai bg status`, then commit again.";

pub fn contended(count: impl Display, sessions: impl Display) -> String {
    format!(
        "refused: this worktree is contended. {count} sessions are open here: {sessions}. End all but one, or run \
         `prov session close <id>`."
    )
}

/// An agent commit while the git-ai service is not available.
pub const GITAI_UNAVAILABLE_AGENT: &str = "refused: the git-ai service is not available, so this commit would lose its attribution. \
     Start it with `git-ai bg start`, and commit where the service is reachable (not in a sandbox that blocks it).";

pub const TAINT_FAILED: &str = "the taint checkpoint failed";
pub const SWEEP_FAILED: &str = "the sweep failed";

pub fn await_failed(error: impl Display) -> String {
    format!("warning: {error}. The checkpoints of this commit may not be applied yet.")
}

pub const NO_MESSAGE_FILE: &str = "prepare-commit-msg needs the message file";

pub fn session_mismatch(open: impl Display, env: impl Display) -> String {
    format!("warning: the open session of this worktree is {open}, but the environment names the session {env}.")
}

pub const CANNOT_READ_SQUASH_MSG: &str = "cannot read SQUASH_MSG";

pub fn squash_refused(agent: impl Display, total: impl Display, example: impl Display, subject: impl Display) -> String {
    format!(
        "refused: a squash of agent commits ({agent} of {total} squashed commits, for example {example} \"{subject}\"). \
         A squash is not permitted: integrate with `git merge <agent branch>`.\nTo stop the squash: git reset --hard"
    )
}

pub fn fast_forward(count: impl Display, branch: impl Display) -> String {
    format!(
        "WARNING: this fast-forward put {count} agent commit(s) on the branch `{branch}` with no merge commit, \
         so the merge gate did not run. A fast-forward of agent commits is not permitted.\n\
         Undo it: git reset --keep ORIG_HEAD\nThen integrate with: git merge --no-ff <agent branch>"
    )
}
