//! `prov approve`.

use std::fmt::Display;

pub const NEEDS_TERMINAL: &str =
    "`prov approve` needs a terminal: only a human can approve, with a typed `yes`. Use --dry-run to see what it will do.";

/// After the question, on the same line as the answer.
pub const TYPE_YES: &str = "Type `yes` to approve: ";

pub fn agent_env(var: impl Display) -> String {
    format!("refused: only a human can approve. {var} is set: an agent started this command.")
}

pub const AGENT_WORKTREE: &str =
    "refused: only a human can approve, and not in an agent worktree. Approve where you merge or commit.";
pub const NO_PATHS: &str = "give one path or more";
pub const AGENT_MODEL_WITHOUT_AI: &str = "--agent and --model go with --ai";
pub const DRY_RUN: &str = "Dry run: nothing changed.";
pub const NOT_APPROVED: &str = "Not approved. Nothing changed.";

// `prov approve --merge`

pub const NO_MERGE: &str =
    "no merge is in progress. Start the merge first: a merge approval exists only in the merge message.";
pub const OCTOPUS: &str = "an octopus merge is in progress. The gate refuses it: git merge --abort";

pub fn no_message_file(path: impl Display) -> String {
    format!("{path} does not exist. Run `git commit` once, so that the gate lists the items.")
}

pub const HEAD_NO_CONFIG: &str = "HEAD has no .provenance.toml";

pub fn nothing_to_approve(path: impl Display) -> String {
    format!("nothing in this merge needs an approval for `{path}`.")
}

/// After [`nothing_to_approve`], on the same line.
pub fn open_items(paths: impl Display) -> String {
    format!(" Open items: {paths}")
}

pub fn path_header(path: impl Display) -> String {
    format!("{path}:")
}

pub fn item(text: impl Display) -> String {
    format!("  {text}")
}

pub const ALREADY_APPROVED: &str = "Each path is already approved in the merge message. Run: git commit";

pub fn confirm_merge(count: impl Display) -> String {
    format!("The lines keep their git-ai attribution. You approve {count} path(s) for this merge.")
}

pub const APPROVED_MERGE: &str = "Approved in the merge message. Run: git commit";

// `prov approve --human` and `--ai`

pub fn not_a_file(path: impl Display) -> String {
    format!("{path} is not a file")
}

pub const GITAI_UNAVAILABLE: &str =
    "the git-ai service is not available, so a checkpoint would record nothing. Start it: git-ai bg start";

/// The author for `--ai`, in [`author`].
pub fn who_ai(agent: impl Display, model: impl Display) -> String {
    format!("an AI (agent `{agent}`, model `{model}`)")
}

/// The author for `--human`, in [`author`].
pub const WHO_HUMAN: &str = "you";

pub fn author(who: impl Display) -> String {
    format!("Every change to these files since the last checkpoint gets the author: {who}.")
}

pub fn path(path: impl Display) -> String {
    format!("  {path}")
}

pub const CONFIRM_OWN: &str = "This applies to each complete file, not to a part of its changes.";

pub fn recorded(choice: impl Display) -> String {
    format!("Recorded. The commit of these files gets `Prov-Approved: <path> {choice}`. A change to a file cancels its approval.")
}
