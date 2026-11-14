//! `prov completions`, and the hint of `prov init`.

use std::fmt::Display;

pub const HOME_NOT_SET: &str = "HOME is not set";
pub const SHELL_NOT_SET: &str = "SHELL is not set. Give --shell.";

pub fn unknown_shell(shell: impl Display) -> String {
    format!("no tab completion for the shell `{shell}`. Give --shell bash, zsh or fish.")
}

/// For `prov init` and `prov doctor`.
pub const NOT_SET_UP: &str = "tab completion is not set up. Run: prov completions --install";

pub fn has_it(file: impl Display) -> String {
    format!("{file} has it already.")
}

pub fn add_line(file: impl Display) -> String {
    format!("add this line to {file}, or run: prov completions --install")
}

pub fn already_in(file: impl Display) -> String {
    format!("already in {file}")
}

pub fn will_add(file: impl Display, line: impl Display) -> String {
    format!("Dry run: will add to {file}: {line}")
}

pub fn added(file: impl Display, line: impl Display) -> String {
    format!("added to {file}: {line}")
}

pub fn open_new_shell(file: impl Display) -> String {
    format!("Open a new shell, or run: source {file}")
}

/// The comment above the line in the startup file.
pub const STARTUP_COMMENT: &str = "# Tab completion for prov";
