//! `prov session list | show | close | drop`.

use anyhow::{bail, Result};
use prov_core::{now, SessionRecord, State, Store};
use serde::Serialize;

use crate::archive;
use crate::context::Ctx;

pub enum Action {
    List,
    Show(String),
    Close(String),
    Drop(String),
}

/// A record with the calculated values.
#[derive(Debug, Serialize)]
pub struct View {
    #[serde(flatten)]
    pub record: SessionRecord,
    /// Calculated when the state is read. Not stored.
    pub stale: bool,
    /// More than one session occupies the worktree.
    pub contended: bool,
}

/// Finds a session by its ID, or by the start of its ID if only one session matches.
pub fn resolve(store: &Store, text: &str) -> Result<SessionRecord> {
    let sessions = store.sessions()?;
    if let Some(exact) = sessions.iter().find(|s| s.id == text) {
        return Ok(exact.clone());
    }
    let matches: Vec<&SessionRecord> = sessions.iter().filter(|s| !text.is_empty() && s.id.starts_with(text)).collect();
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => bail!("no record of a session `{text}`. See: prov session list"),
        many => bail!(
            "`{text}` matches {} sessions: {}",
            many.len(),
            many.iter().map(|s| s.id.as_str()).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// All records, with Closing sessions that cannot finish moved to Failed.
pub fn views(ctx: &Ctx) -> Result<Vec<View>> {
    let now = now();
    let records = ctx.store.lock()?.refresh(now)?;
    Ok(records
        .iter()
        .map(|r| View {
            record: r.clone(),
            stale: r.is_stale(now),
            contended: r.occupies(&r.worktree, now)
                && records.iter().filter(|o| o.occupies(&r.worktree, now)).count() > 1,
        })
        .collect())
}

fn state_text(view: &View) -> String {
    let mut text = view.record.state.as_str().to_string();
    if view.stale {
        text.push_str(", stale");
    }
    if view.contended {
        text.push_str(", contended");
    }
    text
}

fn age(seconds: u64) -> String {
    match seconds {
        s if s < 120 => format!("{s} s"),
        s if s < 2 * 3600 => format!("{} min", s / 60),
        s if s < 48 * 3600 => format!("{} h", s / 3600),
        s => format!("{} d", s / 86400),
    }
}

pub fn run(action: Action, json: bool, dry_run: bool) -> Result<bool> {
    let ctx = Ctx::here()?;
    match action {
        Action::List => {
            let views = views(&ctx)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&views)?);
            } else if views.is_empty() {
                println!("No session has a record. Integrated sessions are in the trailers of their merge commits.");
            } else {
                let now = now();
                for v in &views {
                    println!(
                        "{}:{}  [{}]  {}  last event {} ago\n    {}",
                        v.record.agent,
                        v.record.id,
                        state_text(v),
                        v.record.branch,
                        age(now.saturating_sub(v.record.last_event_at)),
                        v.record.worktree.display(),
                    );
                }
            }
            Ok(true)
        }
        Action::Show(id) => {
            let record = resolve(&ctx.store, &id)?;
            let view = views(&ctx)?.into_iter().find(|v| v.record.id == record.id);
            let Some(view) = view else { bail!("no record of a session `{id}`") };
            if json {
                println!("{}", serde_json::to_string_pretty(&view)?);
            } else {
                let r = &view.record;
                println!("session:    {}:{}", r.agent, r.id);
                println!("state:      {}", state_text(&view));
                println!("branch:     {}", r.branch);
                println!("worktree:   {}", r.worktree.display());
                println!("model:      {}", r.model);
                if let Some(t) = &r.transcript_path {
                    println!("transcript: {}", t.display());
                }
                if let (Some(file), Some(hash)) = (&r.archive_file, &r.archive_sha256) {
                    println!("archive:    {}", file.display());
                    println!("sha256:     {hash} (of the decompressed archive)");
                }
                if let Some(error) = &r.error {
                    println!("error:      {error}");
                }
                println!("last event: {} ago", age(now().saturating_sub(r.last_event_at)));
            }
            Ok(true)
        }
        Action::Close(id) => {
            let record = resolve(&ctx.store, &id)?;
            let after = close(&ctx, &record.id, dry_run)?;
            if json {
                println!("{}", serde_json::json!({"session": record.id, "dry_run": dry_run, "state": after.state.as_str(),
                    "error": after.error, "archive_file": after.archive_file, "archive_sha256": after.archive_sha256}));
            } else if dry_run {
                println!("Dry run: session {} is {}; `prov session close` will run the archive step.", record.id, record.state.as_str());
            } else {
                match after.state {
                    State::Archived => println!(
                        "session {} is archived: {}",
                        after.id,
                        after.archive_file.as_deref().map(|p| p.display().to_string()).unwrap_or_default()
                    ),
                    State::Failed => println!(
                        "session {} is failed: {}\nCorrect the cause, then run the command again.",
                        after.id,
                        after.error.as_deref().unwrap_or("no error text")
                    ),
                    other => println!("session {} is {}", after.id, other.as_str()),
                }
            }
            Ok(dry_run || after.state == State::Archived)
        }
        Action::Drop(id) => {
            let record = resolve(&ctx.store, &id)?;
            if !dry_run {
                ctx.store.lock()?.delete(&record.id)?;
            }
            if json {
                println!("{}", serde_json::json!({"session": record.id, "dry_run": dry_run, "dropped": !dry_run,
                    "archive_file": record.archive_file}));
            } else {
                println!(
                    "{} the record of session {}.{}",
                    if dry_run { "Dry run: will delete" } else { "Deleted" },
                    record.id,
                    match &record.archive_file {
                        Some(file) => format!(" The transcript archive stays: {}", file.display()),
                        None => String::new(),
                    }
                );
                if record.state != State::Archived {
                    println!(
                        "The session was {}: it has no transcript archive, and its branch `{}` can no longer integrate \
                         with a session record. Its lines will be doubtful lines.",
                        record.state.as_str(),
                        record.branch
                    );
                }
            }
            Ok(true)
        }
    }
}

/// Moves an Open or Failed session to Closing and runs the archive step
/// here, not in a detached process. Returns the record after the step.
pub fn close(ctx: &Ctx, id: &str, dry_run: bool) -> Result<SessionRecord> {
    let now = now();
    {
        let lock = ctx.store.lock()?;
        lock.refresh(now)?;
        let Some(mut record) = lock.get(id)? else { bail!("no record of session {id}") };
        match record.state {
            State::Archived => bail!("session {id} is already archived"),
            State::Closing if record.archive_pid.is_some_and(prov_core::state::pid_alive) => {
                bail!("session {id} is closing: its archive step runs now. Wait, then see: prov session show {id}")
            }
            _ => {}
        }
        if dry_run {
            return Ok(record);
        }
        record.close(now)?;
        lock.put(&record)?;
    }
    archive::run_in(ctx, id)?;
    match ctx.store.get(id)? {
        Some(record) => Ok(record),
        None => bail!("the record of session {id} was deleted during the archive step"),
    }
}
