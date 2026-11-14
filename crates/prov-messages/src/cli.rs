//! The command line: errors of the arguments, the error line, and the help
//! of completion candidates. The help texts of the commands and options are
//! the doc comments in `prov/src/main.rs`.

use std::fmt::Display;

pub const APPROVE_MODE: &str = "give exactly one of --merge, --human and --ai";

/// Every error that ends a command.
pub fn error(error: impl Display) -> String {
    format!("prov: error: {error}")
}

/// The help of a session in tab completion.
pub fn session_candidate(branch: impl Display, state: impl Display) -> String {
    format!("{branch} ({state})")
}
