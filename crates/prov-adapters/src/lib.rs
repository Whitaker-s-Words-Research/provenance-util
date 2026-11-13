//! The adapter interface and the Claude Code adapter.
//!
//! An adapter turns the events of one agent into the general events that the
//! session logic handles. The session logic knows no agent. These decisions
//! belong to the adapter, because agents differ in them: which events exist,
//! when the sweep runs, how the model and the transcript are found, and the
//! default commit identity.

pub mod claude_code;

use std::path::{Path, PathBuf};

use anyhow::Result;
use prov_core::Config;

/// What an agent event means for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// The agent says that a session starts or resumes.
    Start,
    /// The session is active.
    Activity,
    /// The agent finished a unit of work and changes nothing until the next event.
    WorkDone,
    /// The session ended.
    End,
}

/// An agent event in general form. The first event of an unknown session
/// records the session, whatever its kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEvent {
    pub kind: EventKind,
    pub session_id: String,
    /// The directory of the session.
    pub cwd: Option<PathBuf>,
    pub transcript_path: Option<PathBuf>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

/// One file of a transcript archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// The name in the archive.
    pub name: String,
    pub source: PathBuf,
}

pub trait Adapter: Sync {
    /// The adapter's name in `prov hook <adapter> <event>`, for example `claude-code`.
    fn adapter_name(&self) -> &'static str;
    /// The agent's name in the config and in trailers. Also git-ai's name for the tool.
    fn agent_name(&self) -> &'static str;
    fn default_identity(&self) -> Identity;
    fn default_command(&self) -> Vec<String>;
    /// Environment variables that the agent sets in the commands it runs.
    fn env_markers(&self) -> &'static [&'static str];
    /// The environment variable with the session ID, if the agent sets one.
    fn session_id_env(&self) -> Option<&'static str>;
    /// The process of the agent, seen from a hook process.
    fn agent_pid(&self) -> Option<u32>;
    fn parse_event(&self, event: &str, payload: &str) -> Result<AgentEvent>;
    /// True if the sweep runs at this event. Every adapter also sweeps in `pre-commit`.
    fn sweeps_at(&self, kind: EventKind) -> bool;
    /// The model of the latest work, if the transcript has one.
    fn latest_model(&self, transcript: &Path) -> Option<String>;
    /// The title that the agent gave the session, if the transcript has one.
    /// Only a label in `prov session list`: no rule depends on it.
    fn title(&self, transcript: &Path) -> Option<String>;
    /// The files of the transcript archive.
    fn archive_entries(&self, session_id: &str, transcript: Option<&Path>) -> Result<Vec<ArchiveEntry>>;
    /// True if text that a hook prints at this event reaches the agent's context.
    fn message_reaches_agent(&self, kind: EventKind) -> bool;
    /// The arguments that make the agent's command resume a session.
    fn resume_args(&self, session_id: &str) -> Vec<String>;
    /// The provider part of a model ID, for a model with no provider: `anthropic`.
    fn model_provider(&self) -> &'static str;
    /// The tracked settings file of the agent, relative to the root of a
    /// worktree. It holds the agent hooks, the commit identity and the
    /// signing setting.
    fn settings_file(&self) -> &'static str;
    /// The settings file with the entries of `prov` merged in. All other keys
    /// stay. `existing` is the text of the file, if it exists.
    fn merge_settings(&self, existing: Option<&str>, identity: &Identity) -> Result<String>;
    /// What the settings file lacks. Empty: the file is complete.
    fn settings_problems(&self, existing: Option<&str>, identity: &Identity) -> Vec<String>;
    /// Checks that git-ai's own integration of this agent is installed for
    /// the user in `home`. `Err` holds what is missing.
    fn gitai_integration(&self, home: &Path) -> std::result::Result<(), String>;
}

/// The environment that makes an agent commit with its identity and without
/// a signature: the author, the committer and `commit.gpgsign=false`.
pub fn commit_env(identity: &Identity) -> Vec<(&'static str, String)> {
    vec![
        ("GIT_AUTHOR_NAME", identity.name.clone()),
        ("GIT_AUTHOR_EMAIL", identity.email.clone()),
        ("GIT_COMMITTER_NAME", identity.name.clone()),
        ("GIT_COMMITTER_EMAIL", identity.email.clone()),
        ("GIT_CONFIG_COUNT", "1".to_string()),
        ("GIT_CONFIG_KEY_0", "commit.gpgsign".to_string()),
        ("GIT_CONFIG_VALUE_0", "false".to_string()),
    ]
}

static ADAPTERS: [&dyn Adapter; 1] = [&claude_code::ClaudeCode];

pub fn all() -> &'static [&'static dyn Adapter] {
    &ADAPTERS
}

pub fn by_adapter_name(name: &str) -> Option<&'static dyn Adapter> {
    all().iter().copied().find(|a| a.adapter_name() == name)
}

pub fn by_agent_name(name: &str) -> Option<&'static dyn Adapter> {
    all().iter().copied().find(|a| a.agent_name() == name)
}

/// The commit identity of an agent: the adapter's default, with the
/// replacements from the `[agent.<name>]` section.
pub fn identity(config: &Config, agent: &str) -> Option<Identity> {
    let default = by_agent_name(agent).map(Adapter::default_identity);
    let section = config.agent_section(agent);
    let name = section.and_then(|s| s.author.clone()).or_else(|| default.as_ref().map(|d| d.name.clone()));
    let email = section.and_then(|s| s.email.clone()).or_else(|| default.as_ref().map(|d| d.email.clone()));
    Some(Identity { name: name.unwrap_or_else(|| agent.to_string()), email: email? })
}

/// The author emails of the agents in the config.
pub fn agent_emails(config: &Config) -> Vec<String> {
    let mut emails: Vec<String> = config
        .agents
        .iter()
        .filter_map(|agent| identity(config, agent))
        .map(|identity| identity.email.to_ascii_lowercase())
        .collect();
    emails.sort();
    emails.dedup();
    emails
}

/// The first agent environment variable that is set, if one is. A command
/// with such a variable was started by an agent.
pub fn agent_env_marker() -> Option<&'static str> {
    all()
        .iter()
        .flat_map(|a| a.env_markers().iter().copied())
        .find(|var| std::env::var_os(var).is_some_and(|v| !v.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_defaults_and_replacements() {
        let config = Config::default();
        let id = identity(&config, "claude").unwrap();
        assert_eq!((id.name.as_str(), id.email.as_str()), ("Claude", "noreply@anthropic.com"));
        assert_eq!(agent_emails(&config), ["noreply@anthropic.com"]);

        let config = Config::parse("agents = [\"claude\", \"other\"]\n[agent.claude]\nemail = \"Bot@Example.invalid\"\n").unwrap();
        assert_eq!(identity(&config, "claude").unwrap().name, "Claude");
        // An agent with no adapter and no email in the config has no identity.
        assert_eq!(agent_emails(&config), ["bot@example.invalid"]);
    }
}
