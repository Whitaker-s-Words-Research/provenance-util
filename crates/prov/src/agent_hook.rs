//! `prov hook <adapter> <event>`: the events of an agent.
//!
//! The logic here knows no agent. The adapter decides what an event means,
//! when the sweep runs and where a message goes.

use anyhow::{bail, Context, Result};
use prov_adapters::EventKind;
use prov_core::{now, SessionRecord, State};
use prov_messages as msg;

use crate::context::Ctx;
use crate::{archive, sweep};

pub fn run(adapter_name: &str, event: &str, payload: &str) -> Result<()> {
    let Some(adapter) = prov_adapters::by_adapter_name(adapter_name) else {
        bail!(msg::agent_hook::unknown_adapter(adapter_name));
    };
    let event = adapter.parse_event(event, payload)?;
    let dir = match &event.cwd {
        Some(dir) => dir.clone(),
        None => std::env::current_dir().context(msg::fs::NO_CURRENT_DIR)?,
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
            out.push(msg::agent_hook::not_agent_worktree(
                ctx.repo.branch.as_deref().unwrap_or(msg::agent_hook::DETACHED_HEAD),
                adapter.agent_name(),
            ));
        }
        out.flush();
        return Ok(());
    }
    let branch = ctx.repo.branch.clone().unwrap_or_default();

    if event.kind == EventKind::Start {
        if let Err(e) = config.check_version(prov_core::VERSION) {
            out.push(msg::agent_hook::version_warning(e));
        }
    }

    let now = now();
    let (session, open_here) = {
        let lock = ctx.store.lock()?;
        // `prov discard` removes a worktree under the lock. A hook that found
        // the worktree before it got the lock must not record a session for
        // it now (R-DISC-1).
        if !ctx.repo.toplevel.is_dir() {
            return Ok(());
        }
        let mut session = match lock.get(&event.session_id)?.map(|s| s.at(now)) {
            Some(mut session) => {
                if event.kind != EventKind::End {
                    // A resumed session returns to Open.
                    session.touch(now);
                }
                // A session finds its record by its worktree.
                session.worktree.clone_from(&ctx.repo.toplevel);
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
            session.model.clone_from(model);
        }
        if event.kind == EventKind::WorkDone {
            if let Some(model) = session.transcript_path.as_deref().and_then(|t| adapter.latest_model(t)) {
                session.model = model;
            }
        }
        if event.kind == EventKind::End {
            // The sweep at the end: for the session that ends, while it is
            // still the only occupant. A session can end with no Stop, and its
            // changes would then have no checkpoint.
            if session.state == State::Open {
                let occupants = lock.open_in(&ctx.repo.toplevel)?;
                let swept = sweep::observe(&ctx, &lock, &occupants).and_then(|()| {
                    if matches!(occupants.as_slice(), [only] if only.id == session.id) {
                        sweep::sweep(&ctx, &session)?;
                    }
                    Ok(())
                });
                if let Err(e) = swept {
                    out.push(msg::agent_hook::sweep_failed(format!("{e:#}")));
                }
            }
            if matches!(session.state, State::Open | State::Failed) {
                session.last_event_at = now;
                session.close(now)?;
                lock.put(&session)?;
                // The session no longer occupies the worktree. If one session
                // is left, it becomes the owner now, and what is changed at
                // this moment is tainted: it can be the work of either.
                // Without this, the next event of that session would taint
                // its own later work.
                // With no session left, the next hook observes as before: the
                // end sweep has just run, and this hook has little time.
                let remaining = lock.open_in(&ctx.repo.toplevel)?;
                if !remaining.is_empty() {
                    if let Err(e) = sweep::observe(&ctx, &lock, &remaining) {
                        out.push(msg::agent_hook::warning(format!("{e:#}")));
                    }
                }
                // The agent gives this hook little time: a detached process does the archive step.
                match archive::spawn_detached(&ctx, &session.id, session.generation) {
                    Ok(pid) => session.archive_pid = Some(pid),
                    Err(e) => session.failed(msg::agent_hook::cannot_start_archive(format!("{e:#}"))),
                }
                lock.put(&session)?;
            }
            // An End of a session that is not Open changes nothing.
            out.flush();
            return Ok(());
        }
        lock.put(&session)?;
        let open_here = lock.open_in(&ctx.repo.toplevel)?;
        // Every hook looks at the worktree first: a change of the occupant taints what is changed now.
        if let Err(e) = sweep::observe(&ctx, &lock, &open_here) {
            out.push(msg::agent_hook::warning(format!("{e:#}")));
        }
        (session, open_here)
    };

    if open_here.len() > 1 {
        // prov cannot stop the second session. The worktree is contended.
        if event.kind == EventKind::Start {
            let others: Vec<&str> = open_here.iter().filter(|s| s.id != session.id).map(|s| s.id.as_str()).collect();
            out.push(msg::agent_hook::contended(others.join(", "), adapter.agent_name()));
        }
    }

    if event.kind == EventKind::Start && !ctx.gitai.service_available(&ctx.repo.toplevel) {
        // Without the service, a checkpoint exits 0 and records nothing.
        out.push(msg::agent_hook::GITAI_UNAVAILABLE);
        ctx.gitai.start_service(&ctx.repo.toplevel);
    }

    if adapter.sweeps_at(event.kind) && open_here.len() == 1 {
        if let Err(e) = sweep::sweep(&ctx, &session) {
            out.push(msg::agent_hook::sweep_failed(format!("{e:#}")));
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
