//! `.provenance.toml`: four keys and the optional `[agent.<name>]` sections.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::pattern::PathSet;

/// Name of the config file in the repository root.
pub const CONFIG_FILE: &str = ".provenance.toml";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// A semver requirement, for example `=0.1.0`. `None`: no requirement.
    #[serde(default)]
    pub required_version: Option<String>,
    #[serde(default = "default_agents")]
    pub agents: Vec<String>,
    /// Relative paths resolve against the base directory.
    #[serde(default = "default_logs_dir")]
    pub logs_dir: String,
    #[serde(default = "default_protected")]
    pub protected: Vec<String>,
    #[serde(default)]
    pub agent: BTreeMap<String, AgentSection>,
}

/// Replaces defaults of an adapter.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSection {
    pub author: Option<String>,
    pub email: Option<String>,
    pub command: Option<Vec<String>>,
}

fn default_agents() -> Vec<String> {
    vec!["claude".into()]
}
fn default_logs_dir() -> String {
    "llm-logs".into()
}
fn default_protected() -> Vec<String> {
    vec![".provenance.toml".into(), ".claude/**".into(), ".github/workflows/**".into()]
}

impl Default for Config {
    fn default() -> Self {
        Self {
            required_version: None,
            agents: default_agents(),
            logs_dir: default_logs_dir(),
            protected: default_protected(),
            agent: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Self> {
        let config: Config = toml::from_str(text).with_context(|| format!("{CONFIG_FILE} does not parse"))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if let Some(req) = &self.required_version {
            semver::VersionReq::parse(req)
                .with_context(|| format!("{CONFIG_FILE}: invalid required_version `{req}`"))?;
        }
        self.protected_set()?;
        Ok(())
    }

    /// `Ok(())` if `version` satisfies `required_version`; otherwise an error
    /// that names both.
    pub fn check_version(&self, version: &str) -> Result<()> {
        let Some(req) = &self.required_version else { return Ok(()) };
        let parsed_req = semver::VersionReq::parse(req)
            .with_context(|| format!("{CONFIG_FILE}: invalid required_version `{req}`"))?;
        let parsed = semver::Version::parse(version).with_context(|| format!("invalid version `{version}`"))?;
        if parsed_req.matches(&parsed) {
            Ok(())
        } else {
            anyhow::bail!("prov {version} does not satisfy required_version = \"{req}\"")
        }
    }

    pub fn protected_set(&self) -> Result<PathSet> {
        PathSet::new(&self.protected).with_context(|| format!("{CONFIG_FILE}: protected"))
    }

    pub fn agent_section(&self, agent: &str) -> Option<&AgentSection> {
        self.agent.get(agent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_example_from_the_spec() {
        let c = Config::parse(
            r#"
required_version = "=0.1.0"
agents = ["claude"]
logs_dir = "llm-logs"
protected = [".provenance.toml", ".claude/**", ".github/workflows/**"]

[agent.claude]
command = ["claude", "--model", "opus"]
"#,
        )
        .unwrap();
        assert_eq!(c.agent["claude"].command.as_ref().unwrap().len(), 3);
        assert!(c.check_version("0.1.0").is_ok());
        assert!(c.check_version("0.1.1").is_err());
    }

    #[test]
    fn defaults_and_unknown_keys() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
        assert!(Config::parse("nonsense = 1").is_err());
        assert!(Config::parse("required_version = \"not a version\"").is_err());
        assert!(Config::default().check_version("9.9.9").is_ok());
    }
}
