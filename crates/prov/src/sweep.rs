//! The sweep. In an agent worktree with one Open session, every change is the
//! session's change: an `agent-v1` checkpoint of type `ai_agent` on the
//! changed files gives them to the session.

use anyhow::Result;
use prov_core::SessionRecord;

use crate::context::Ctx;

/// Returns the number of files in the checkpoint. The caller checks that
/// exactly one session is Open in the worktree.
pub fn sweep(ctx: &Ctx, session: &SessionRecord) -> Result<usize> {
    let files = ctx.repo.changed_files()?;
    if files.is_empty() {
        return Ok(0);
    }
    // The agent name is also git-ai's name for the tool, and the conversation
    // ID is the session ID: the checkpoint joins the git-ai session of the
    // agent's own hooks. A git-ai session has one model, so the model must be right.
    ctx.gitai.checkpoint_ai(&ctx.repo.toplevel, &files, &session.agent, &session.model, &session.id)?;
    Ok(files.len())
}
