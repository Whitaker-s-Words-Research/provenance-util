//! Live state in `.git/provenance/` and the session state machine.
//!
//! Stored states: Open, Closing, Archived, Failed. Stale is calculated when
//! the state is read. Every write happens under one advisory file lock.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use prov_messages as msg;
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
    /// Increases by 1 each time the session enters Open or Closing. An
    /// archive process writes its result only for its own generation.
    #[serde(default)]
    pub generation: u64,
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
            generation: 1,
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
            self.generation += 1;
            self.archive_file = None;
            self.archive_sha256 = None;
            self.archive_pid = None;
            self.closing_at = None;
            self.error = None;
        }
        self.last_event_at = now;
    }

    /// Open -> Closing. Also Failed -> Closing, to run the archive step
    /// again, and Closing -> Closing, when its archive process is gone. Each
    /// is a new generation: an archive process of an earlier one writes nothing.
    pub fn close(&mut self, now: u64) -> Result<()> {
        match self.state {
            State::Open | State::Failed | State::Closing => {
                self.state = State::Closing;
                self.generation += 1;
                self.closing_at = Some(now);
                self.archive_pid = None;
                self.error = None;
                Ok(())
            }
            State::Archived => bail!(msg::state::already_archived(&self.id)),
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

    /// The record as it is at `now`: a Closing session whose archive step can
    /// no longer finish is Failed. Calculated when read, like stale, never
    /// stored. Reason: no process then writes the record, so this rule needs
    /// no generation check and cannot race with an archive step or a resume.
    #[must_use]
    pub fn at(mut self, now: u64) -> Self {
        if self.closing_timed_out(now) {
            self.state = State::Failed;
            self.error = Some(msg::state::ARCHIVE_TIMED_OUT.into());
        }
        self
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

/// An approval with `prov approve --human` or `--ai`, kept until the commit.
/// A merge approval is not stored: it exists only in the merge message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRecord {
    pub worktree: PathBuf,
    /// Relative to the root of the worktree.
    pub path: String,
    /// `human` or `ai`.
    pub choice: String,
    /// The git blob ID of the file content at the approval. A change to the
    /// file cancels the approval.
    pub content_hash: String,
    pub approved_at: u64,
}

impl ApprovalRecord {
    /// The name of the record: one approval for each path of a worktree.
    pub fn key(&self) -> String {
        approval_key(&self.worktree, &self.path)
    }
}

fn approval_key(worktree: &Path, path: &str) -> String {
    // FNV-1a, 64 bit, of the worktree and the path. Only a file name: a
    // collision would replace one pending approval with another.
    let mut hash = [0xcbf2_9ce4_8422_2325_u64, 0x8422_2325_cbf2_9ce4_u64];
    for (seed, i) in hash.iter_mut().zip(0u8..) {
        for byte in worktree.as_os_str().as_encoded_bytes().iter().chain([0u8, i].iter()).chain(path.as_bytes()) {
            *seed ^= u64::from(*byte);
            *seed = seed.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{:016x}{:016x}", hash[0], hash[1])
}

/// What `prov` knows of one agent worktree between hooks (R-SWEEP-2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeRecord {
    pub worktree: PathBuf,
    /// The session that `prov` last saw as the only occupant.
    #[serde(default)]
    pub owner: Option<String>,
}

impl WorktreeRecord {
    /// One hook's look at the worktree. `occupants` are the IDs of the Open
    /// sessions that are not stale. If they are not exactly the owner, the
    /// changes that the worktree holds now are tainted: `taint` gives them a
    /// checkpoint that names no session. Then the owner becomes the single
    /// occupant, or none. Returns true if the record changed.
    ///
    /// Reason: `prov` cannot see who made a change. When the occupant
    /// changes, the changes before that can belong to the session before.
    /// If `taint` fails, the owner stays, and the next hook tries again.
    pub fn observe(&mut self, occupants: &[&str], taint: impl FnOnce() -> Result<()>) -> Result<bool> {
        if let (Some(owner), [only]) = (&self.owner, occupants) {
            if owner == only {
                return Ok(false);
            }
        }
        taint()?;
        let owner = match occupants {
            [only] => Some(only.to_string()),
            _ => None,
        };
        let changed = self.owner != owner;
        self.owner = owner;
        Ok(changed)
    }
}

/// One copy of `prov cp`: the plan, the blob ID of each copied file and the
/// check result of each file. It stays until the copy check agrees for every file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyRecord {
    /// The plan ID.
    pub id: String,
    pub worktree: PathBuf,
    pub created_at: u64,
    /// The `<source>` of the `Copied-From` trailers: the remote URL of the
    /// source repository, or its absolute path.
    pub source: String,
    pub source_commit: String,
    pub files: Vec<CopyFile>,
    /// True if the copy check failed for a file. `prov doctor` reports it.
    #[serde(default)]
    pub failed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyFile {
    /// Relative to the root of the source.
    pub path: String,
    /// Relative to the root of the worktree.
    pub dest: String,
    /// The git blob ID of the copied bytes.
    pub blob: String,
    /// The git file mode: `100644`, `100755` or `120000`.
    pub mode: String,
    /// False for a binary file and a symbolic link: no line authorship.
    pub text: bool,
    pub ranges: Vec<crate::plan::Range>,
    /// The commit that holds the file. `None` until the human commits it.
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub committed_at: Option<u64>,
    /// The result of the copy check.
    #[serde(default)]
    pub check: Option<CopyCheck>,
    /// What the check found, for `moved` and `failed`.
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CopyCheck {
    /// Each line has the author of the plan.
    Agrees,
    /// Authors moved between identical lines. The counts agree.
    Moved,
    /// git-ai lists no line of the file, because it ignores the path. The
    /// file has no line authorship.
    Ignored,
    Failed,
}

impl CopyRecord {
    /// Sets `failed` from the files. Returns true if the record is done:
    /// every file is checked and none failed or moved.
    pub fn settle(&mut self) -> bool {
        self.failed = self.files.iter().any(|f| f.check == Some(CopyCheck::Failed));
        self.files.iter().all(|f| f.check == Some(CopyCheck::Agrees))
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

    fn approvals_dir(&self) -> PathBuf {
        self.dir.join("approvals")
    }

    /// The pending approvals of one worktree, sorted by path.
    pub fn approvals(&self, worktree: &Path) -> Result<Vec<ApprovalRecord>> {
        let mut out = Vec::new();
        let entries = match fs::read_dir(self.approvals_dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e).context(msg::state::CANNOT_READ_APPROVALS),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match fs::read(&path) {
                Ok(bytes) => {
                    let record: ApprovalRecord =
                        serde_json::from_slice(&bytes).with_context(|| msg::fs::does_not_parse(path.display()))?;
                    if record.worktree == worktree {
                        out.push(record);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| msg::fs::cannot_read(path.display())),
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    fn worktree_path(&self, worktree: &Path) -> PathBuf {
        self.dir.join("worktrees").join(format!("{}.json", approval_key(worktree, "")))
    }

    /// The record of one agent worktree. A worktree with no record has no
    /// owner and no tainted file.
    pub fn worktree(&self, worktree: &Path) -> Result<WorktreeRecord> {
        let path = self.worktree_path(worktree);
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| msg::fs::does_not_parse(path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(WorktreeRecord { worktree: worktree.to_path_buf(), owner: None })
            }
            Err(e) => Err(e).with_context(|| msg::fs::cannot_read(path.display())),
        }
    }

    /// The records of all agent worktrees.
    pub fn worktree_records(&self) -> Result<Vec<WorktreeRecord>> {
        let mut out = Vec::new();
        let entries = match fs::read_dir(self.dir.join("worktrees")) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e).context(msg::state::CANNOT_READ_WORKTREES),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match fs::read(&path) {
                Ok(bytes) => out.push(
                    serde_json::from_slice(&bytes).with_context(|| msg::fs::does_not_parse(path.display()))?,
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| msg::fs::cannot_read(path.display())),
            }
        }
        Ok(out)
    }

    fn copies_dir(&self) -> PathBuf {
        self.dir.join("copies")
    }

    /// The copy records of all worktrees, oldest first.
    pub fn copies(&self) -> Result<Vec<CopyRecord>> {
        let mut out = Vec::new();
        let entries = match fs::read_dir(self.copies_dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e).context(msg::state::CANNOT_READ_COPIES),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match fs::read(&path) {
                Ok(bytes) => out.push(
                    serde_json::from_slice::<CopyRecord>(&bytes)
                        .with_context(|| msg::fs::does_not_parse(path.display()))?,
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| msg::fs::cannot_read(path.display())),
            }
        }
        out.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        Ok(out)
    }

    /// The path of the record of one copy.
    pub fn copy_path(&self, id: &str) -> Result<PathBuf> {
        check_id(id)?;
        Ok(self.copies_dir().join(format!("{id}.json")))
    }

    /// Takes the advisory lock. All writes go through the returned guard.
    pub fn lock(&self) -> Result<Locked<'_>> {
        fs::create_dir_all(self.sessions_dir()).with_context(|| msg::fs::cannot_create(self.dir.display()))?;
        let path = self.dir.join("lock");
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| msg::fs::cannot_open(path.display()))?;
        file.lock().with_context(|| msg::fs::cannot_lock(path.display()))?;
        Ok(Locked { store: self, _file: file })
    }

    /// Reads one record without the lock. Writes are atomic renames, so a
    /// reader sees a complete file.
    pub fn get(&self, id: &str) -> Result<Option<SessionRecord>> {
        let path = self.session_path(id)?;
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(
                serde_json::from_slice(&bytes).with_context(|| msg::fs::does_not_parse(path.display()))?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| msg::fs::cannot_read(path.display())),
        }
    }

    /// All records, in start order.
    pub fn sessions(&self) -> Result<Vec<SessionRecord>> {
        let mut out = Vec::new();
        let entries = match fs::read_dir(self.sessions_dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e).context(msg::state::CANNOT_READ_SESSIONS),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match fs::read(&path) {
                Ok(bytes) => out.push(
                    serde_json::from_slice::<SessionRecord>(&bytes)
                        .with_context(|| msg::fs::does_not_parse(path.display()))?,
                ),
                // Deleted between the listing and the read.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| msg::fs::cannot_read(path.display())),
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
        let mut file = File::create(&tmp).with_context(|| msg::fs::cannot_write(tmp.display()))?;
        file.write_all(&serde_json::to_vec_pretty(record)?)?;
        file.write_all(b"\n")?;
        drop(file);
        fs::rename(&tmp, &path).with_context(|| msg::fs::cannot_write(path.display()))
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        let path = self.store.session_path(id)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).with_context(|| msg::fs::cannot_delete(path.display())),
        }
    }

    pub fn put_approval(&self, record: &ApprovalRecord) -> Result<()> {
        let dir = self.store.approvals_dir();
        fs::create_dir_all(&dir).with_context(|| msg::fs::cannot_create(dir.display()))?;
        let path = dir.join(format!("{}.json", record.key()));
        let tmp = path.with_extension("json.tmp");
        let mut file = File::create(&tmp).with_context(|| msg::fs::cannot_write(tmp.display()))?;
        file.write_all(&serde_json::to_vec_pretty(record)?)?;
        file.write_all(b"\n")?;
        drop(file);
        fs::rename(&tmp, &path).with_context(|| msg::fs::cannot_write(path.display()))
    }

    pub fn delete_approval(&self, record: &ApprovalRecord) -> Result<bool> {
        let path = self.store.approvals_dir().join(format!("{}.json", record.key()));
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).with_context(|| msg::fs::cannot_delete(path.display())),
        }
    }

    pub fn copies(&self) -> Result<Vec<CopyRecord>> {
        self.store.copies()
    }

    pub fn worktree(&self, worktree: &Path) -> Result<WorktreeRecord> {
        self.store.worktree(worktree)
    }

    pub fn put_worktree(&self, record: &WorktreeRecord) -> Result<()> {
        let path = self.store.worktree_path(&record.worktree);
        let dir = path.parent().context(msg::state::NO_DIRECTORY)?;
        fs::create_dir_all(dir).with_context(|| msg::fs::cannot_create(dir.display()))?;
        let tmp = path.with_extension("json.tmp");
        let mut file = File::create(&tmp).with_context(|| msg::fs::cannot_write(tmp.display()))?;
        file.write_all(&serde_json::to_vec_pretty(record)?)?;
        file.write_all(b"\n")?;
        drop(file);
        fs::rename(&tmp, &path).with_context(|| msg::fs::cannot_write(path.display()))
    }

    pub fn delete_worktree(&self, worktree: &Path) -> Result<()> {
        match fs::remove_file(self.store.worktree_path(worktree)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).context(msg::state::CANNOT_DELETE_WORKTREE),
        }
    }

    pub fn put_copy(&self, record: &CopyRecord) -> Result<()> {
        let path = self.store.copy_path(&record.id)?;
        let dir = self.store.copies_dir();
        fs::create_dir_all(&dir).with_context(|| msg::fs::cannot_create(dir.display()))?;
        let tmp = path.with_extension("json.tmp");
        let mut file = File::create(&tmp).with_context(|| msg::fs::cannot_write(tmp.display()))?;
        file.write_all(&serde_json::to_vec_pretty(record)?)?;
        file.write_all(b"\n")?;
        drop(file);
        fs::rename(&tmp, &path).with_context(|| msg::fs::cannot_write(path.display()))
    }

    pub fn delete_copy(&self, id: &str) -> Result<bool> {
        let path = self.store.copy_path(id)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).with_context(|| msg::fs::cannot_delete(path.display())),
        }
    }

    /// All records as they are at `now` (see [`SessionRecord::at`]), in start
    /// order. Nothing is written.
    pub fn sessions_at(&self, now: u64) -> Result<Vec<SessionRecord>> {
        Ok(self.sessions()?.into_iter().map(|s| s.at(now)).collect())
    }
}

/// True if `id` can be the name of a record.
pub fn is_record_name(id: &str) -> bool {
    check_id(id).is_ok()
}

/// A session ID becomes a file name.
fn check_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 200
        && !id.starts_with('.')
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        bail!(msg::state::invalid_session_id(id));
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
        // Open, Closing, Closing again, Open: each is a new generation.
        assert_eq!(s.generation, 4);
        s.touch(600); // an event of an Open session is not one
        assert_eq!(s.generation, 4);
    }

    #[test]
    fn a_change_of_occupant_taints_the_changes() {
        let mut w = WorktreeRecord { worktree: PathBuf::from("/w/a"), owner: None };
        let count = std::cell::Cell::new(0);
        let taint = || {
            count.set(count.get() + 1);
            Ok(())
        };
        let never = || -> Result<()> { panic!("the owner is the only occupant: nothing is tainted") };
        // The first session becomes the owner.
        assert!(w.observe(&["a"], taint).unwrap());
        assert_eq!((w.owner.as_deref(), count.get()), (Some("a"), 1));
        assert!(!w.observe(&["a"], never).unwrap());
        // The owner is gone, and another session is there.
        assert!(w.observe(&["b"], taint).unwrap());
        assert_eq!((w.owner.as_deref(), count.get()), (Some("b"), 2));
        // Contended: no owner. Each hook taints what is changed then.
        assert!(w.observe(&["b", "c"], taint).unwrap());
        assert!(!w.observe(&["b", "c"], taint).unwrap());
        assert_eq!((w.owner.as_deref(), count.get()), (None, 4));
        // No occupant: the same.
        assert!(!w.observe(&[], taint).unwrap());
        assert_eq!(count.get(), 5);
        // A taint that fails leaves the owner, so the next hook tries again.
        assert!(w.observe(&["b"], || anyhow::bail!("no service")).is_err());
        assert_eq!(w.owner, None);
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
        assert_eq!(store.sessions().unwrap().len(), 0);
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
        assert_eq!(lock.sessions_at(10 + CLOSING_TIMEOUT_SECS - 1).unwrap()[0].state, State::Closing);
        assert_eq!(lock.sessions_at(10 + CLOSING_TIMEOUT_SECS).unwrap()[0].state, State::Failed);
        // Calculated, never stored.
        assert_eq!(store.get("s1").unwrap().unwrap().state, State::Closing);
    }

    #[test]
    fn approvals_are_kept_for_each_worktree_and_path() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let approval = |worktree: &str, path: &str, choice: &str| ApprovalRecord {
            worktree: PathBuf::from(worktree),
            path: path.into(),
            choice: choice.into(),
            content_hash: "abc".into(),
            approved_at: 1,
        };
        assert_eq!(store.approvals(Path::new("/w/a")).unwrap().len(), 0);
        let lock = store.lock().unwrap();
        lock.put_approval(&approval("/w/a", "b.txt", "human")).unwrap();
        lock.put_approval(&approval("/w/a", "a.txt", "human")).unwrap();
        lock.put_approval(&approval("/w/a", "a.txt", "ai")).unwrap(); // replaces
        lock.put_approval(&approval("/w/b", "a.txt", "human")).unwrap();
        let here = store.approvals(Path::new("/w/a")).unwrap();
        assert_eq!(here.iter().map(|a| (a.path.as_str(), a.choice.as_str())).collect::<Vec<_>>(), [("a.txt", "ai"), ("b.txt", "human")]);
        assert!(lock.delete_approval(&here[0]).unwrap());
        assert!(!lock.delete_approval(&here[0]).unwrap());
        assert_eq!(store.approvals(Path::new("/w/a")).unwrap().len(), 1);
        assert_eq!(store.approvals(Path::new("/w/b")).unwrap().len(), 1);
    }
}
