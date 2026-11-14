//! `prov session`: list, show, close and drop.

use std::fmt::Display;

pub fn no_match(text: impl Display) -> String {
    format!("no record of a session `{text}`. See: prov session list")
}

pub fn ambiguous(text: impl Display, count: impl Display, ids: impl Display) -> String {
    format!("`{text}` matches {count} sessions: {ids}")
}

/// Added to the state of a session.
pub const STALE: &str = ", stale";
pub const CONTENDED: &str = ", contended";

/// An age: seconds, minutes, hours or days.
pub fn age_seconds(n: impl Display) -> String {
    format!("{n} s")
}

pub fn age_minutes(n: impl Display) -> String {
    format!("{n} min")
}

pub fn age_hours(n: impl Display) -> String {
    format!("{n} h")
}

pub fn age_days(n: impl Display) -> String {
    format!("{n} d")
}

// `prov session list`

pub const NONE: &str = "No session has a record. Integrated sessions are in the trailers of their merge commits.";

/// The first line of a session. The title follows, if there is one.
pub fn list_line(agent: impl Display, id: impl Display, state: impl Display, age: impl Display) -> String {
    format!("{agent}  {id}  {state}  {age} ago")
}

pub fn list_location(branch: impl Display, worktree: impl Display) -> String {
    format!("    {branch}   {worktree}")
}

// `prov session show`

pub fn no_record(id: impl Display) -> String {
    format!("no record of a session `{id}`")
}

pub fn show_session(agent: impl Display, id: impl Display) -> String {
    format!("session:    {agent}:{id}")
}

pub fn show_title(title: impl Display) -> String {
    format!("title:      {title}")
}

pub fn show_state(state: impl Display) -> String {
    format!("state:      {state}")
}

pub fn show_branch(branch: impl Display) -> String {
    format!("branch:     {branch}")
}

pub fn show_worktree(worktree: impl Display) -> String {
    format!("worktree:   {worktree}")
}

pub fn show_model(model: impl Display) -> String {
    format!("model:      {model}")
}

pub fn show_transcript(path: impl Display) -> String {
    format!("transcript: {path}")
}

pub fn show_archive(file: impl Display) -> String {
    format!("archive:    {file}")
}

pub fn show_sha256(hash: impl Display) -> String {
    format!("sha256:     {hash} (of the decompressed archive)")
}

pub fn show_error(error: impl Display) -> String {
    format!("error:      {error}")
}

pub fn show_last_event(age: impl Display) -> String {
    format!("last event: {age} ago")
}

// `prov session archive`

pub fn archive_dry_run(id: impl Display, state: impl Display) -> String {
    format!("Dry run: session {id} is {state}; `prov session archive` will run the archive step.")
}

pub fn archived(id: impl Display, file: impl Display) -> String {
    format!("session {id} is archived: {file}")
}

pub fn failed(id: impl Display, error: impl Display) -> String {
    format!("session {id} is failed: {error}\nCorrect the cause, then run the command again.")
}

/// A Failed session with no error text.
pub const NO_ERROR_TEXT: &str = "no error text";

pub fn state(id: impl Display, state: impl Display) -> String {
    format!("session {id} is {state}")
}

/// After the list of processes that still run in the worktree.
pub const STOP_PROCESSES: &str = "Stop them before another session starts there.";

pub fn no_record_of(id: impl Display) -> String {
    format!("no record of session {id}")
}

pub fn closing_now(id: impl Display) -> String {
    format!("session {id} is closing: its archive step runs now. Wait, then see: prov session show {id}")
}

pub fn no_agent_pid(id: impl Display) -> String {
    format!(
        "warning: prov does not know the process ID of the agent of session {id}, so it archives the \
         transcript now. If the agent still works, the archive misses its last work, until the next event \
         of the agent opens the session again. End the agent first if it runs."
    )
}

pub fn resumed(id: impl Display, state: impl Display) -> String {
    format!(
        "the agent of session {id} sent another event while prov waited, so the session went on \
         (now {state}). It is archived when the agent ends. See: prov session show {id}"
    )
}

pub fn deleted_during_archive(id: impl Display) -> String {
    format!("the record of session {id} was deleted during the archive step")
}

// `prov session drop`

pub fn will_drop(id: impl Display) -> String {
    format!("Dry run: will delete the record of session {id}.")
}

pub fn dropped(id: impl Display) -> String {
    format!("Deleted the record of session {id}.")
}

/// After [`will_drop`] or [`dropped`], on the same line.
pub fn archive_stays(file: impl Display) -> String {
    format!(" The transcript archive stays: {file}")
}

pub fn dropped_unarchived(state: impl Display, branch: impl Display) -> String {
    format!(
        "The session was {state}: it has no transcript archive, and its branch `{branch}` can no longer integrate \
         with a session record. Its lines will be doubtful lines."
    )
}
