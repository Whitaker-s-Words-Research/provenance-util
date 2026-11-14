//! The Claude Code adapter: its settings file, hook payloads and transcripts.

use std::fmt::Display;

pub const SETTINGS_DO_NOT_PARSE: &str = ".claude/settings.json does not parse";
pub const SETTINGS_NOT_OBJECT: &str = ".claude/settings.json is not a JSON object";
pub const SETTINGS_MISSING: &str = ".claude/settings.json does not exist";

pub fn unknown_event(event: impl Display) -> String {
    format!("unknown Claude Code event `{event}`")
}

pub const PAYLOAD_DOES_NOT_PARSE: &str = "the hook payload does not parse";
pub const PAYLOAD_NO_SESSION: &str = "the hook payload has no session_id";
pub const NO_TRANSCRIPT_PATH: &str = "the session has no transcript path";

pub fn transcript_missing(path: impl Display) -> String {
    format!("the transcript {path} does not exist")
}

pub const ENV_NOT_OBJECT: &str = "`env` in .claude/settings.json is not an object";
pub const HOOKS_NOT_OBJECT: &str = "`hooks` in .claude/settings.json is not an object";

pub fn hooks_event_not_array(event: impl Display) -> String {
    format!("`hooks.{event}` in .claude/settings.json is not an array")
}

/// A problem that `prov doctor` reports: a wrong `env` value.
///
/// `prov init` finds these problems by the start `` `env. `` and the words
/// `is not set`. Keep both when you change the text.
pub fn env_wrong(key: impl Display, found: Option<impl Display>, value: impl Display) -> String {
    match found {
        Some(found) => format!("`env.{key}` is `{found}`, not `{value}`"),
        None => format!("`env.{key}` is not set, not `{value}`"),
    }
}

/// A problem that `prov doctor` reports: a missing hook.
pub fn hook_missing(command: impl Display, event: impl Display) -> String {
    format!("no hook `{command}` for the event {event}")
}

pub fn settings_does_not_parse(path: impl Display, error: impl Display) -> String {
    format!("{path} does not parse: {error}")
}

pub fn gitai_hook_missing(path: impl Display, event: impl Display) -> String {
    format!("{path} has no `git-ai checkpoint` hook for {event}. Run: git-ai install-hooks")
}

pub fn file_name_not_utf8(dir: impl Display) -> String {
    format!("a file name in {dir} is not UTF-8")
}
