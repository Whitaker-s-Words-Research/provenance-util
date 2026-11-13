//! `prov session list | show | close | drop`.

use anyhow::{bail, Result};
use prov_core::{now, SessionRecord, State, Store};
use serde::Serialize;

use crate::archive;
use crate::context::{say, Ctx};

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
        [] => bail!("no record of a session `{text}`. See: prov session list"),
        many => bail!(
            "`{text}` matches {} sessions: {}",
            many.len(),
            many.iter().map(|s| s.id.as_str()).collect::<Vec<_>>().join(", ")
        ),
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
        text.push_str(", stale");
    }
    if view.contended {
        text.push_str(", contended");
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
            let mut views = views(&ctx)?;
            views.iter_mut().for_each(View::read_title);
            if json {
                println!("{}", serde_json::to_string_pretty(&views)?);
            } else if views.is_empty() {
                println!("No session has a record. Integrated sessions are in the trailers of their merge commits.");
            } else {
                let now = now();
                let ids: Vec<&str> = views.iter().map(|v| v.record.id.as_str()).collect();
                for (v, id) in views.iter().zip(short_ids(&ids)) {
                    let mut line = format!(
                        "{}  {id}  {}  {} ago",
                        v.record.agent,
                        state_text(v),
                        age(now.saturating_sub(v.record.last_event_at))
                    );
                    if let Some(title) = &v.title {
                        line.push_str("  ");
                        line.push_str(title);
                    }
                    println!("{line}\n    {}   {}", v.record.branch, tilde(&v.record.worktree));
                }
            }
            Ok(true)
        }
        Action::Show(id) => {
            let record = resolve(&ctx.store, &id)?;
            let view = views(&ctx)?.into_iter().find(|v| v.record.id == record.id);
            let Some(mut view) = view else { bail!("no record of a session `{id}`") };
            view.read_title();
            if json {
                println!("{}", serde_json::to_string_pretty(&view)?);
            } else {
                let r = &view.record;
                println!("session:    {}:{}", r.agent, r.id);
                if let Some(title) = &view.title {
                    println!("title:      {title}");
                }
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
                        after.archive_file.as_deref().map_or_default(|p| p.display().to_string())
                    ),
                    State::Failed => println!(
                        "session {} is failed: {}\nCorrect the cause, then run the command again.",
                        after.id,
                        after.error.as_deref().unwrap_or("no error text")
                    ),
                    other => println!("session {} is {}", after.id, other.as_str()),
                }
            }
            if !dry_run && after.worktree.is_dir() {
                crate::procs::warn_if_busy(&after.worktree, "Stop them before another session starts there.");
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
    let generation = {
        let lock = ctx.store.lock()?;
        let Some(mut record) = lock.get(id)?.map(|r| r.at(now)) else { bail!("no record of session {id}") };
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
        if record.agent_pid.is_none() {
            // R-SESS-3, accepted limit: the step cannot wait for the agent.
            say(format!(
                "warning: prov does not know the process ID of the agent of session {id}, so it archives the \
                 transcript now. If the agent still works, the archive misses its last work, until the next event \
                 of the agent opens the session again (R-SESS-3). End the agent first if it runs."
            ));
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
                say(format!(
                    "the agent of session {id} sent another event while prov waited, so the session went on \
                     (now {}). It is archived when the agent ends. See: prov session show {id}",
                    record.state.as_str()
                ));
            }
            Ok(record)
        }
        None => bail!("the record of session {id} was deleted during the archive step"),
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
