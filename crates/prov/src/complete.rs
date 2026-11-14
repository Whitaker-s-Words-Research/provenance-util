//! Tab completion: `source <(COMPLETE=bash prov)` in the shell's startup
//! file. The shell asks `prov` itself for the candidates, so branch names and
//! session IDs come from the repository of the current directory.
//!
//! A completer never fails: with no repository, or any error, it offers
//! nothing.

use clap_complete::engine::CompletionCandidate;
use prov_core::pattern::is_agent_branch;

use crate::context::Ctx;

fn ctx() -> Option<Ctx> {
    Ctx::here().ok()
}

fn local_branches(ctx: &Ctx) -> Vec<String> {
    ctx.repo
        .git(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .map_or_default(|out| out.lines().map(str::to_string).collect())
}

/// Every local branch.
pub fn branches() -> Vec<CompletionCandidate> {
    let Some(ctx) = ctx() else { return Vec::new() };
    local_branches(&ctx).into_iter().map(CompletionCandidate::new).collect()
}

/// The branches that match `agent/**`.
pub fn agent_branches() -> Vec<CompletionCandidate> {
    let Some(ctx) = ctx() else { return Vec::new() };
    local_branches(&ctx).into_iter().filter(|b| is_agent_branch(b)).map(CompletionCandidate::new).collect()
}

fn session_candidates(ctx: &Ctx) -> Vec<CompletionCandidate> {
    ctx.store
        .sessions()
        .unwrap_or_default()
        .into_iter()
        .map(|s| CompletionCandidate::new(s.id).help(Some(prov_messages::cli::session_candidate(&s.branch, s.state.as_str()).into())))
        .collect()
}

/// The session IDs that have a record.
pub fn sessions() -> Vec<CompletionCandidate> {
    let Some(ctx) = ctx() else { return Vec::new() };
    session_candidates(&ctx)
}

/// For `prov finish`: the branches of the linked worktrees, then the sessions.
pub fn finish_targets() -> Vec<CompletionCandidate> {
    let Some(ctx) = ctx() else { return Vec::new() };
    let mut out: Vec<CompletionCandidate> = ctx
        .repo
        .worktrees()
        .unwrap_or_default()
        .into_iter()
        // The first one is the main worktree, which `prov finish` refuses.
        .skip(1)
        .filter_map(|w| w.branch.map(|b| CompletionCandidate::new(b).help(Some(w.path.display().to_string().into()))))
        .collect();
    out.extend(session_candidates(&ctx));
    out
}

/// The agents of the config file.
pub fn agents() -> Vec<CompletionCandidate> {
    let Some(ctx) = ctx() else { return Vec::new() };
    ctx.repo.config_here().map_or_default(|c| c.agents).into_iter().map(CompletionCandidate::new).collect()
}
