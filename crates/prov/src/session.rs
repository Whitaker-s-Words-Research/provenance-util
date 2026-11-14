//! `prov session list | show | archive | drop`.

use anyhow::{bail, Result};
use prov_messages as msg;
use prov_core::{now, SessionRecord, State, Store};
use serde::Serialize;

use crate::archive;
use crate::context::{say, Ctx};

pub enum Action {
    List,
    Show(String),
    Archive(String),
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
    /// The title that the agent gave the session. Read from the transcript
    /// by `list` and `show` only. Not stored.
    pub title: Option<String>,
}

impl View {
    /// Reads the title from the transcript, if the agent has one there.
    fn read_title(&mut self) {
        self.title = self
            .record
            .transcript_path
            .as_deref()
            .zip(prov_adapters::by_agent_name(&self.record.agent))
            .and_then(|(transcript, adapter)| adapter.title(transcript));
    }
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
        [] => bail!(msg::session::no_match(text)),
        many => bail!(msg::session::ambiguous(
            text,
            many.len(),
            many.iter().map(|s| s.id.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// All records, as they are now: a Closing session that cannot finish is Failed.
pub fn views(ctx: &Ctx) -> Result<Vec<View>> {
    let now = now();
    let records = ctx.store.lock()?.sessions_at(now)?;
    Ok(records
        .iter()
        .map(|r| View {
            record: r.clone(),
            stale: r.is_stale(now),
            contended: r.occupies(&r.worktree, now)
                && records.iter().filter(|o| o.occupies(&r.worktree, now)).count() > 1,
            title: None,
        })
        .collect())
}

fn state_text(view: &View) -> String {
    let mut text = view.record.state.as_str().to_string();
    if view.stale {
        text.push_str(msg::session::STALE);
    }
    if view.contended {
        text.push_str(msg::session::CONTENDED);
    }
    text
}

/// The start of each ID that tells it apart from the others: 8 characters
/// or more. `resolve` accepts it.
fn short_ids<'a>(ids: &[&'a str]) -> Vec<&'a str> {
    ids.iter()
        .map(|id| {
            let shared = ids
                .iter()
                .filter(|other| *other != id)
                .map(|other| id.chars().zip(other.chars()).take_while(|(a, b)| a == b).count())
                .max()
                .unwrap_or(0);
            let chars = shared.max(7) + 1;
            id.char_indices().nth(chars).map_or(*id, |(end, _)| &id[..end])
        })
        .collect()
}

/// The path with `~` for the home directory.
fn tilde(path: &std::path::Path) -> String {
    match std::env::var_os("HOME").map(std::path::PathBuf::from) {
        Some(home) if home.as_os_str().len() > 1 => match path.strip_prefix(&home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        },
        _ => path.display().to_string(),
    }
}

fn age(seconds: u64) -> String {
    match seconds {
        s if s < 120 => msg::session::age_seconds(s),
        s if s < 2 * 3600 => msg::session::age_minutes(s / 60),
        s if s < 48 * 3600 => msg::session::age_hours(s / 3600),
        s => msg::session::age_days(s / 86400),
    }
}

pub fn run(action: Action, json: bool, dry_run: bool) -> Result<bool> {
    let ctx = Ctx::here()?;
    match action {
        Action::List => {
            let mut views = views(&ctx)?;
            views.iter_mut().for_each(View::read_title);
            if json {
                println!("{}", serde_json::to_string_pretty(&views)?);
            } else if views.is_empty() {
                println!("{}", msg::session::NONE);
            } else {
                let now = now();
                let ids: Vec<&str> = views.iter().map(|v| v.record.id.as_str()).collect();
                for (v, id) in views.iter().zip(short_ids(&ids)) {
                    let mut line =
                        msg::session::list_line(&v.record.agent, id, state_text(v), age(now.saturating_sub(v.record.last_event_at)));
                    if let Some(title) = &v.title {
                        line.push_str("  ");
                        line.push_str(title);
                    }
                    println!("{line}\n{}", msg::session::list_location(&v.record.branch, tilde(&v.record.worktree)));
                }
            }
            Ok(true)
        }
        Action::Show(id) => {
            let record = resolve(&ctx.store, &id)?;
            let view = views(&ctx)?.into_iter().find(|v| v.record.id == record.id);
            let Some(mut view) = view else { bail!(msg::session::no_record(&id)) };
            view.read_title();
            if json {
                println!("{}", serde_json::to_string_pretty(&view)?);
            } else {
                let r = &view.record;
                println!("{}", msg::session::show_session(&r.agent, &r.id));
                if let Some(title) = &view.title {
                    println!("{}", msg::session::show_title(title));
                }
                println!("{}", msg::session::show_state(state_text(&view)));
                println!("{}", msg::session::show_branch(&r.branch));
                println!("{}", msg::session::show_worktree(r.worktree.display()));
                println!("{}", msg::session::show_model(&r.model));
                if let Some(t) = &r.transcript_path {
                    println!("{}", msg::session::show_transcript(t.display()));
                }
                if let (Some(file), Some(hash)) = (&r.archive_file, &r.archive_sha256) {
                    println!("{}", msg::session::show_archive(file.display()));
                    println!("{}", msg::session::show_sha256(hash));
                }
                if let Some(error) = &r.error {
                    println!("{}", msg::session::show_error(error));
                }
                println!("{}", msg::session::show_last_event(age(now().saturating_sub(r.last_event_at))));
            }
            Ok(true)
        }
        Action::Archive(id) => {
            let record = resolve(&ctx.store, &id)?;
            let after = close(&ctx, &record.id, dry_run)?;
            if json {
                println!("{}", serde_json::json!({"session": record.id, "dry_run": dry_run, "state": after.state.as_str(),
                    "error": after.error, "archive_file": after.archive_file, "archive_sha256": after.archive_sha256}));
            } else if dry_run {
                println!("{}", msg::session::archive_dry_run(&record.id, record.state.as_str()));
            } else {
                match after.state {
                    State::Archived => println!(
                        "{}",
                        msg::session::archived(&after.id, after.archive_file.as_deref().map_or_default(|p| p.display().to_string()))
                    ),
                    State::Failed => println!(
                        "{}",
                        msg::session::failed(&after.id, after.error.as_deref().unwrap_or(msg::session::NO_ERROR_TEXT))
                    ),
                    other => println!("{}", msg::session::state(&after.id, other.as_str())),
                }
            }
            if !dry_run && after.worktree.is_dir() {
                crate::procs::warn_if_busy(&after.worktree, msg::session::STOP_PROCESSES);
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
                let mut line = if dry_run { msg::session::will_drop(&record.id) } else { msg::session::dropped(&record.id) };
                if let Some(file) = &record.archive_file {
                    line.push_str(&msg::session::archive_stays(file.display()));
                }
                println!("{line}");
                if record.state != State::Archived {
                    println!("{}", msg::session::dropped_unarchived(record.state.as_str(), &record.branch));
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
    let generation = {
        let lock = ctx.store.lock()?;
        let Some(mut record) = lock.get(id)?.map(|r| r.at(now)) else { bail!(msg::session::no_record_of(id)) };
        match record.state {
            State::Archived => bail!(msg::state::already_archived(id)),
            State::Closing if record.archive_pid.is_some_and(prov_core::state::pid_alive) => {
                bail!(msg::session::closing_now(id))
            }
            _ => {}
        }
        if dry_run {
            return Ok(record);
        }
        if record.agent_pid.is_none() {
            // R-SESS-3, accepted limit: the step cannot wait for the agent.
            say(msg::session::no_agent_pid(id));
        }
        record.close(now)?;
        lock.put(&record)?;
        record.generation
    };
    // In the foreground, with a message if the step waits for the agent process.
    archive::run_in(ctx, id, None, true)?;
    match ctx.store.get(id)? {
        Some(record) => {
            if record.generation != generation {
                // R-SESS-1: an event of the agent opened the session again while
                // the step waited, and this step stopped without a change.
                say(msg::session::resumed(id, record.state.as_str()));
            }
            Ok(record)
        }
        None => bail!(msg::session::deleted_during_archive(id)),
    }
}

#[cfg(test)]
mod tests {
    use super::short_ids;

    #[test]
    fn short_ids_tell_the_sessions_apart() {
        let ids = ["5d26531d-b3ff-4ea3", "5d26531d-c000-1111", "abcdef0123456789", "short"];
        assert_eq!(short_ids(&ids), ["5d26531d-b", "5d26531d-c", "abcdef01", "short"]);
        assert_eq!(short_ids(&["x"]), ["x"]);
    }
}
