//! The archive step of a session: the transcript archive.

use std::fmt::Display;

pub const NO_BINARY: &str = "cannot find the prov binary";
pub const CANNOT_SPAWN: &str = "cannot start the archive process";

pub fn no_record(session: impl Display) -> String {
    format!("no record of session {session}")
}

pub fn not_closing(session: impl Display, state: impl Display) -> String {
    format!("session {session} is {state}, not closing")
}

pub fn waiting(session: impl Display, pid: impl Display) -> String {
    format!(
        "session {session}: waiting for the agent process {pid} to exit. The transcript is archived after that. \
         End the agent, or stop this command with Ctrl-C; the session then stays closing."
    )
}

pub fn no_adapter(agent: impl Display) -> String {
    format!("no adapter for the agent `{agent}`")
}

pub fn cannot_add(name: impl Display) -> String {
    format!("cannot add {name} to the archive")
}

pub const CANNOT_FINISH: &str = "cannot finish the archive";

pub fn other_file(file: impl Display, hash: impl Display) -> String {
    format!("{file} exists and is not the archive with the hash {hash}. prov never overwrites an archive file: move it away.")
}
