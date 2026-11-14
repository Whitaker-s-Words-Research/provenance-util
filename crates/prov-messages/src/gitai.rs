//! Errors of git-ai commands.

use std::fmt::Display;

pub const UNEXPECTED_DIFF: &str = "unexpected output of git-ai diff --json";
pub const UNEXPECTED_BLAME: &str = "unexpected output of git-ai blame --json";

pub fn undefined_prompt(key: impl Display) -> String {
    format!("git-ai blame names the prompt {key} and does not define it")
}

pub fn blame_range(part: impl Display) -> String {
    format!("git-ai blame has the line range `{part}`")
}

pub const NO_STDIN: &str = "no stdin";

pub fn checkpoint_failed(stderr: impl Display) -> String {
    format!("git-ai checkpoint failed: {stderr}")
}

pub fn blame_failed(file: impl Display, stderr: impl Display) -> String {
    format!("git-ai blame {file} failed: {stderr}")
}

/// The context of an error in the blame of one file.
pub fn in_file(file: impl Display) -> String {
    format!("file {file}")
}

pub const VERSION_FAILED: &str = "git-ai --version failed";
pub const NO_VERSION: &str = "git-ai --version printed no version";

pub fn config_failed(key: impl Display, stderr: impl Display) -> String {
    format!("git-ai config {key} failed: {stderr}")
}

pub fn unexpected_config(key: impl Display) -> String {
    format!("unexpected output of git-ai config {key}")
}

pub fn cannot_run(error: impl Display) -> String {
    format!("cannot run git-ai: {error}")
}

pub fn await_timed_out(seconds: impl Display, stderr: impl Display) -> String {
    format!("git-ai await did not finish in {seconds} seconds: {stderr}")
}

pub fn diff_failed(commit: impl Display, stderr: impl Display) -> String {
    format!("git-ai diff {commit} failed: {stderr}")
}

/// The context of an error in the authorship of one commit.
pub fn in_commit(commit: impl Display) -> String {
    format!("commit {commit}")
}
