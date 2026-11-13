//! The sweep, and taint.
//!
//! In an agent worktree with one occupant, every change with no checkpoint
//! is the occupant's change: an `agent-v1` checkpoint of type `ai_agent` on
//! the changed files gives them to the session.
//!
//! That is true only while the occupant stays the same. Before a sweep, each
//! hook looks at the worktree ([`observe`]). When the occupant is not the
//! session that `prov` saw there last, the changes that the worktree holds
//! at that moment are tainted: they get an AI checkpoint under the tool
//! `prov-unattributed`, with a new conversation ID. No record and no trailer
//! names that session, so the gate lists its lines and a human decides.
//! After the approval, the note still says "AI, session unknown".
//!
//! Leaving the changes with no checkpoint is not enough: git-ai gives a line
//! with no checkpoint in an agent commit to an AI session of that commit. A
//! `known_human` checkpoint would make the gate list the lines too, but the
//! note would then say that a human wrote them.

use anyhow::Result;
use prov_core::state::Locked;
use prov_core::SessionRecord;

use prov_gitai::UNATTRIBUTED_TOOL;

use crate::context::Ctx;

/// One hook's look at an agent worktree, under the lock. `occupants` are the
/// Open sessions of the worktree that are not stale.
pub fn observe(ctx: &Ctx, lock: &Locked<'_>, occupants: &[SessionRecord]) -> Result<()> {
    let top = &ctx.repo.toplevel;
    let mut record = lock.worktree(top)?;
    let ids: Vec<&str> = occupants.iter().map(|s| s.id.as_str()).collect();
    let taint = || -> Result<()> {
        let files = ctx.repo.changed_files()?;
        if files.is_empty() {
            return Ok(());
        }
        // Lines that git-ai already has a checkpoint for keep their author:
        // a checkpoint covers only the changes since the one before.
        ctx.gitai.checkpoint_ai(top, &files, UNATTRIBUTED_TOOL, "unknown", &taint_id())
    };
    if record.observe(&ids, taint)? {
        lock.put_worktree(&record)?;
    }
    Ok(())
}

/// A conversation ID that no other taint has: the time and the process.
fn taint_id() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("taint-{nanos}-{}", std::process::id())
}

/// Returns the number of files in the checkpoint. The caller checks that
/// `session` is the only occupant of the worktree, after [`observe`].
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
