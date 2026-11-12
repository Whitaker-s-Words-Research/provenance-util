//! Live state in `.git/provenance/` and the session state machine.
//!
//! Stored states: Open, Closing, Archived, Failed. Stale is calculated when
//! the state is read. Every write happens under one advisory file lock.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// An Open session with no event for this long is stale.
pub const STALE_AFTER_SECS: u64 = 24 * 60 * 60;
/// A Closing session with no live archive process becomes Failed after this.
pub const CLOSING_TIMEOUT_SECS: u64 = 5 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Open,
    Closing,
    Archived,
    Failed,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Open => "open",
            State::Closing => "closing",
            State::Archived => "archived",
            State::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    /// The agent's name, for example `claude`.
    pub agent: String,
    pub state: State,
    pub worktree: PathBuf,
    pub branch: String,
    pub model: String,
    #[serde(default)]
    pub transcript_path: Option<PathBuf>,
    #[serde(default)]
    pub archive_file: Option<PathBuf>,
    #[serde(default)]
    pub archive_sha256: Option<String>,
    /// The error text of a Failed session.
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub agent_pid: Option<u32>,
    /// The detached archive process of a Closing session.
    #[serde(default)]
    pub archive_pid: Option<u32>,
    pub started_at: u64,
    pub last_event_at: u64,
    #[serde(default)]
    pub closing_at: Option<u64>,
    pub prov_version: String,
}

impl SessionRecord {
    pub fn new(id: &str, agent: &str, worktree: &Path, branch: &str, now: u64) -> Self {
        Self {
            id: id.to_string(),
            agent: agent.to_string(),
            state: State::Open,
            worktree: worktree.to_path_buf(),
            branch: branch.to_string(),
            model: "unknown".to_string(),
            transcript_path: None,
            archive_file: None,
            archive_sha256: None,
            error: None,
            agent_pid: None,
            archive_pid: None,
            started_at: now,
            last_event_at: now,
            closing_at: None,
            prov_version: crate::VERSION.to_string(),
        }
    }

    /// An event of the agent arrived. A session in any other state returns to
    /// Open (a resumed session); the results of an earlier archive are cleared.
    pub fn touch(&mut self, now: u64) {
        if self.state != State::Open {
            self.state = State::Open;
            self.archive_file = None;
            self.archive_sha256 = None;
            self.archive_pid = None;
            self.closing_at = None;
            self.error = None;
        }
        self.last_event_at = now;
    }

    /// Open -> Closing. Also Failed -> Closing, to run the archive step again.
    pub fn close(&mut self, now: u64) -> Result<()> {
        match self.state {
            State::Open | State::Failed => {
                self.state = State::Closing;
                self.closing_at = Some(now);
                self.archive_pid = None;
                self.error = None;
                Ok(())
            }
            State::Closing => Ok(()),
            State::Archived => bail!("session {} is already archived", self.id),
        }
    }

    pub fn archived(&mut self, file: PathBuf, sha256: String) {
        self.state = State::Archived;
        self.archive_file = Some(file);
        self.archive_sha256 = Some(sha256);
        self.archive_pid = None;
        self.error = None;
    }

    pub fn failed(&mut self, error: impl Into<String>) {
        self.state = State::Failed;
        self.error = Some(error.into());
        self.archive_pid = None;
    }

    /// Calculated, never stored: an Open session with no event for 24 hours,
    /// or whose agent process does not exist.
    pub fn is_stale(&self, now: u64) -> bool {
        self.state == State::Open
            && (now.saturating_sub(self.last_event_at) >= STALE_AFTER_SECS
                || self.agent_pid.is_some_and(|pid| !pid_alive(pid)))
    }

    /// A Closing session whose archive step can no longer finish.
    fn closing_timed_out(&self, now: u64) -> bool {
        self.state == State::Closing
            && now.saturating_sub(self.closing_at.unwrap_or(self.last_event_at)) >= CLOSING_TIMEOUT_SECS
            && !self.archive_pid.is_some_and(pid_alive)
    }

    /// Only an Open session that is not stale occupies a worktree. A stale
    /// session stays Open, and it must still be closed before its branch
    /// integrates, but it does not make the worktree contended.
    pub fn occupies(&self, worktree: &Path, now: u64) -> bool {
        self.state == State::Open && self.worktree == worktree && !self.is_stale(now)
    }
}

/// True if a process with this ID exists.
pub fn pid_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else { return false };
    if pid <= 0 {
        return false;
    }
    // Signal 0 checks for existence. EPERM means the process exists.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// `.git/provenance/` in the common git directory.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(common_git_dir: &Path) -> Self {
        Self { dir: common_git_dir.join("provenance") }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn sessions_dir(&self) -> PathBuf {
        self.dir.join("sessions")
    }

    fn session_path(&self, id: &str) -> Result<PathBuf> {
        check_id(id)?;
        Ok(self.sessions_dir().join(format!("{id}.json")))
    }

    /// Takes the advisory lock. All writes go through the returned guard.
    pub fn lock(&self) -> Result<Locked<'_>> {
        fs::create_dir_all(self.sessions_dir()).with_context(|| format!("cannot create {}", self.dir.display()))?;
        let path = self.dir.join("lock");
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        file.lock().with_context(|| format!("cannot lock {}", path.display()))?;
        Ok(Locked { store: self, _file: file })
    }

    /// Reads one record without the lock. Writes are atomic renames, so a
    /// reader sees a complete file.
    pub fn get(&self, id: &str) -> Result<Option<SessionRecord>> {
        let path = self.session_path(id)?;
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(
                serde_json::from_slice(&bytes).with_context(|| format!("{} does not parse", path.display()))?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    /// All records, in start order.
    pub fn sessions(&self) -> Result<Vec<SessionRecord>> {
        let mut out = Vec::new();
        let entries = match fs::read_dir(self.sessions_dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e).context("cannot read the session records"),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match fs::read(&path) {
                Ok(bytes) => out.push(
                    serde_json::from_slice::<SessionRecord>(&bytes)
                        .with_context(|| format!("{} does not parse", path.display()))?,
                ),
                // Deleted between the listing and the read.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
            }
        }
        out.sort_by(|a, b| (a.started_at, &a.id).cmp(&(b.started_at, &b.id)));
        Ok(out)
    }

    /// The sessions that occupy a worktree: Open and not stale. More than
    /// one: the worktree is contended.
    pub fn open_in(&self, worktree: &Path) -> Result<Vec<SessionRecord>> {
        let now = crate::now();
        Ok(self.sessions()?.into_iter().filter(|s| s.occupies(worktree, now)).collect())
    }
}

/// The store with the lock held.
pub struct Locked<'a> {
    store: &'a Store,
    _file: File,
}

impl Locked<'_> {
    pub fn get(&self, id: &str) -> Result<Option<SessionRecord>> {
        self.store.get(id)
    }

    pub fn sessions(&self) -> Result<Vec<SessionRecord>> {
        self.store.sessions()
    }

    pub fn open_in(&self, worktree: &Path) -> Result<Vec<SessionRecord>> {
        self.store.open_in(worktree)
    }

    pub fn put(&self, record: &SessionRecord) -> Result<()> {
        let path = self.store.session_path(&record.id)?;
        let tmp = path.with_extension("json.tmp");
        let mut file = File::create(&tmp).with_context(|| format!("cannot write {}", tmp.display()))?;
        file.write_all(&serde_json::to_vec_pretty(record)?)?;
        file.write_all(b"\n")?;
        drop(file);
        fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        let path = self.store.session_path(id)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).with_context(|| format!("cannot delete {}", path.display())),
        }
    }

    /// Moves each Closing session whose archive step can no longer finish to
    /// Failed. Returns all records, in start order.
    pub fn refresh(&self, now: u64) -> Result<Vec<SessionRecord>> {
        let mut sessions = self.sessions()?;
        for s in &mut sessions {
            if s.closing_timed_out(now) {
                s.failed("the archive step did not finish in 5 minutes");
                self.put(s)?;
            }
        }
        Ok(sessions)
    }
}

/// A session ID becomes a file name.
fn check_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 200
        && !id.starts_with('.')
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        bail!("invalid session ID `{id}`");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, now: u64) -> SessionRecord {
        SessionRecord::new(id, "claude", Path::new("/w/a"), "agent/claude/x", now)
    }

    #[test]
    fn state_machine() {
        let mut s = record("s1", 100);
        assert_eq!(s.state, State::Open);
        s.close(200).unwrap();
        assert_eq!(s.state, State::Closing);
        s.failed("no transcript");
        assert_eq!((s.state, s.error.as_deref()), (State::Failed, Some("no transcript")));
        s.close(300).unwrap(); // run the archive step again
        s.archived(PathBuf::from("/logs/x.tar.gz"), "ab".repeat(32));
        assert_eq!(s.state, State::Archived);
        assert!(s.close(400).is_err());
        s.touch(500); // resumed
        assert_eq!(s.state, State::Open);
        assert!(s.archive_sha256.is_none());
    }

    #[test]
    fn stale_is_calculated() {
        let mut s = record("s1", 1000);
        assert!(!s.is_stale(1000 + STALE_AFTER_SECS - 1));
        assert!(s.is_stale(1000 + STALE_AFTER_SECS));
        s.agent_pid = Some(std::process::id());
        assert!(!s.is_stale(1001));
        s.agent_pid = Some(i32::MAX as u32); // no such process
        assert!(s.is_stale(1001));
        s.close(1002).unwrap();
        assert!(!s.is_stale(1003)); // only an Open session is stale
    }

    #[test]
    fn store_round_trip_and_contention() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        assert!(store.sessions().unwrap().is_empty());
        {
            let lock = store.lock().unwrap();
            let now = crate::now();
            lock.put(&record("b", now + 2)).unwrap();
            lock.put(&record("a", now + 1)).unwrap();
            let mut closing = record("c", now + 3);
            closing.close(now + 3).unwrap();
            lock.put(&closing).unwrap();
            // Open, with no event for a long time: stale.
            lock.put(&record("d", now.saturating_sub(STALE_AFTER_SECS))).unwrap();
        }
        let ids: Vec<_> = store.sessions().unwrap().into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["d", "a", "b", "c"]);
        // A Closing session and a stale session do not occupy the worktree.
        assert_eq!(store.open_in(Path::new("/w/a")).unwrap().len(), 2);
        assert_eq!(store.open_in(Path::new("/w/other")).unwrap().len(), 0);
        let lock = store.lock().unwrap();
        assert!(lock.delete("a").unwrap());
        assert!(!lock.delete("a").unwrap());
        assert!(store.get("a").unwrap().is_none());
        assert!(store.get("../x").is_err());
    }

    #[test]
    fn closing_becomes_failed_after_five_minutes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let lock = store.lock().unwrap();
        let mut s = record("s1", 0);
        s.close(10).unwrap();
        lock.put(&s).unwrap();
        assert_eq!(lock.refresh(10 + CLOSING_TIMEOUT_SECS - 1).unwrap()[0].state, State::Closing);
        assert_eq!(lock.refresh(10 + CLOSING_TIMEOUT_SECS).unwrap()[0].state, State::Failed);
        assert_eq!(store.get("s1").unwrap().unwrap().state, State::Failed);
    }
}
