//! What every command needs: the worktree, live state and git-ai.

use std::path::Path;

use anyhow::{Context, Result};
use prov_core::pattern::is_agent_branch;
use prov_core::Store;
use prov_git::Repo;
use prov_gitai::GitAi;

pub struct Ctx {
    pub repo: Repo,
    pub store: Store,
    pub gitai: GitAi,
}

impl Ctx {
    /// `None` if `dir` is not in a worktree.
    pub fn discover(dir: &Path) -> Result<Option<Ctx>> {
        let Some(repo) = Repo::discover(dir)? else { return Ok(None) };
        let store = Store::new(&repo.common_dir);
        Ok(Some(Ctx { repo, store, gitai: GitAi::default() }))
    }

    pub fn here() -> Result<Ctx> {
        let dir = std::env::current_dir().context("no current directory")?;
        Ctx::discover(&dir)?.context("not in a git worktree")
    }

    /// True if the checked-out branch matches `agent/**`.
    pub fn on_agent_branch(&self) -> bool {
        self.repo.branch.as_deref().is_some_and(is_agent_branch)
    }
}

/// `<version>+<build SHA>`, the value of `Prov-Version`.
pub fn full_version() -> String {
    format!("{}+{}", prov_core::VERSION, env!("PROV_BUILD_SHA"))
}

/// A message for the person at the terminal.
pub fn say(text: impl AsRef<str>) {
    for line in text.as_ref().lines() {
        eprintln!("prov: {line}");
    }
}
