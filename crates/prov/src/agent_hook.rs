//! `prov hook <adapter> <event>`: the events of an agent.
//!
//! The logic here knows no agent. The adapter decides what an event means,
//! when the sweep runs and where a message goes.

use anyhow::{bail, Context, Result};
use prov_adapters::EventKind;
use prov_core::{now, SessionRecord, State};

use crate::context::Ctx;
use crate::{archive, sweep};

pub fn run(adapter_name: &str, event: &str, payload: &str) -> Result<()> {
    let Some(adapter) = prov_adapters::by_adapter_name(adapter_name) else {
        bail!("unknown adapter `{adapter_name}`");
    };
    let event = adapter.parse_event(event, payload)?;
    let dir = match &event.cwd {
        Some(dir) => dir.clone(),
        None => std::env::current_dir().context("no current directory")?,
    };
    // A session outside a git repository is not a session for prov.
    let Some(ctx) = Ctx::discover(&dir)? else { return Ok(()) };
    let config = ctx.repo.config_here()?;
    if !config.agents.iter().any(|a| a == adapter.agent_name()) {
        return Ok(());
    }

    let mut out = Messages { to_agent: adapter.message_reaches_agent(event.kind), lines: Vec::new() };

    if !ctx.on_agent_branch() {
        // No record, no transcript archive and no sweep outside an agent worktree.
        if event.kind == EventKind::Start {
            out.push(format!(
                "prov: this session is not in an agent worktree (branch: {}). prov records no session here, \
                 and it refuses a commit by an agent on this branch. Agent work reaches the branch `{}` only \
                 through a merge of an agent branch. Stop, and ask the human to start the session with \
                 `prov start {}` or in a worktree on a branch that matches `agent/**`.",
                ctx.repo.branch.as_deref().unwrap_or("detached HEAD"),
                config.human_branch,
                adapter.agent_name(),
            ));
        }
        out.flush();
        return Ok(());
    }
    let branch = ctx.repo.branch.clone().unwrap_or_default();

    if event.kind == EventKind::Start {
        if let Err(e) = config.check_version(prov_core::VERSION) {
            out.push(format!("prov: warning: {e}."));
        }
    }

    let now = now();
    let (session, open_here) = {
        let lock = ctx.store.lock()?;
        lock.refresh(now)?;
        let mut session = match lock.get(&event.session_id)? {
            Some(mut session) => {
                if event.kind != EventKind::End {
                    // A resumed session returns to Open.
                    session.touch(now);
                }
                // A session finds its record by its worktree.
                session.worktree = ctx.repo.toplevel.clone();
                session.branch = branch;
                session
            }
            // The first event records the session, whatever the event is.
            None => SessionRecord::new(&event.session_id, adapter.agent_name(), &ctx.repo.toplevel, &branch, now),
        };
        if let Some(pid) = adapter.agent_pid() {
            session.agent_pid = Some(pid);
        }
        if let Some(path) = &event.transcript_path {
            session.transcript_path = Some(path.clone());
        }
        if let Some(model) = &event.model {
            session.model = model.clone();
        }
        if event.kind == EventKind::WorkDone {
            if let Some(model) = session.transcript_path.as_deref().and_then(|t| adapter.latest_model(t)) {
                session.model = model;
            }
        }
        if event.kind == EventKind::End && matches!(session.state, State::Open | State::Failed) {
            session.last_event_at = now;
            session.close(now)?;
            lock.put(&session)?;
            // The agent gives this hook little time: a detached process does the archive step.
            match archive::spawn_detached(&ctx, &session.id) {
                Ok(pid) => session.archive_pid = Some(pid),
                Err(e) => session.failed(format!("cannot start the archive step: {e:#}")),
            }
        }
        lock.put(&session)?;
        let open_here = lock.open_in(&ctx.repo.toplevel)?;
        (session, open_here)
    };

    if event.kind == EventKind::End {
        return Ok(());
    }

    if open_here.len() > 1 {
        // prov cannot stop the second session. The worktree is contended.
        if event.kind == EventKind::Start {
            let others: Vec<&str> = open_here.iter().filter(|s| s.id != session.id).map(|s| s.id.as_str()).collect();
            out.push(format!(
                "prov: STOP. This agent worktree already has an open session ({}). An agent worktree holds one \
                 open session at a time. While more than one is open, prov runs no sweep here and refuses every \
                 commit. Change no files. Tell the human to end the other session or to run \
                 `prov session close <id>`, or to start this session in its own worktree with `prov start {}`.",
                others.join(", "),
                adapter.agent_name(),
            ));
        }
    }

    if event.kind == EventKind::Start && !ctx.gitai.service_available(&ctx.repo.toplevel) {
        // Without the service, a checkpoint exits 0 and records nothing.
        out.push(
            "prov: warning: the git-ai service is not available. prov started it. While it is not available, \
             prov refuses commits in this worktree. Check with `git-ai bg status`.",
        );
        ctx.gitai.start_service(&ctx.repo.toplevel);
    }

    if adapter.sweeps_at(event.kind) && open_here.len() == 1 {
        if let Err(e) = sweep::sweep(&ctx, &session) {
            out.push(format!("prov: warning: the sweep failed: {e:#}"));
        }
    }

    out.flush();
    Ok(())
}

/// Messages of one hook run. Where an event's output reaches the agent, they
/// go to standard output; elsewhere to standard error, so that they never
/// enter the agent's context by accident.
struct Messages {
    to_agent: bool,
    lines: Vec<String>,
}

impl Messages {
    fn push(&mut self, text: impl Into<String>) {
        self.lines.push(text.into());
    }

    fn flush(&self) {
        for line in &self.lines {
            if self.to_agent {
                println!("{line}");
            } else {
                eprintln!("{line}");
            }
        }
    }
}
