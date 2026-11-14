//! The archive step: a deterministic tar of the transcript, its SHA-256
//! before compression, and the gzip file in `logs_dir`.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use prov_messages as msg;
use flate2::{Compression, GzBuilder};
use prov_adapters::ArchiveEntry;
use prov_core::state::pid_alive;
use prov_core::State;
use sha2::{Digest, Sha256};

use crate::context::Ctx;

/// Starts `prov archive-session <id>` in its own process session, so that it
/// continues when the agent and its hook process exit.
pub fn spawn_detached(ctx: &Ctx, session_id: &str, generation: u64) -> Result<u32> {
    let exe = std::env::current_exe().context(msg::archive::NO_BINARY)?;
    let mut cmd = Command::new(exe);
    cmd.args(["archive-session", session_id, "--generation", &generation.to_string()])
        .current_dir(&ctx.repo.toplevel);
    crate::detach::detach(&mut cmd);
    let child = cmd.spawn().context(msg::archive::CANNOT_SPAWN)?;
    Ok(child.id())
}

/// The archive step of one Closing session, for the detached process.
pub fn run(session_id: &str, generation: Option<u64>) -> Result<()> {
    run_in(&Ctx::here()?, session_id, generation, false)
}

/// The archive step of one Closing session. The result is in the record:
/// Archived, or Failed with the error text.
///
/// The step belongs to one generation of the session (`generation`, or the
/// one of the record at the start). It writes its result only if the session
/// is still Closing in that generation. Reason: the session can be resumed,
/// do more work and end again while this step runs.
///
/// `announce` prints what the step waits for, for a person at a terminal.
pub fn run_in(ctx: &Ctx, session_id: &str, generation: Option<u64>, announce: bool) -> Result<()> {
    let session = {
        let lock = ctx.store.lock()?;
        let Some(mut session) = lock.get(session_id)? else { bail!(msg::archive::no_record(session_id)) };
        if session.state != State::Closing {
            bail!(msg::archive::not_closing(session_id, session.state.as_str()));
        }
        if generation.is_some_and(|g| g != session.generation) {
            // The session moved on. Its newer archive process does the work.
            return Ok(());
        }
        session.archive_pid = Some(std::process::id());
        lock.put(&session)?;
        session
    };
    let current = |ctx: &Ctx| -> Result<bool> {
        Ok(ctx.store.get(session_id)?.is_some_and(|r| r.state == State::Closing && r.generation == session.generation))
    };

    // Wait until the agent process has exited: until then, the transcript
    // can grow. If prov does not know the process, it cannot wait.
    if let Some(pid) = session.agent_pid.filter(|pid| pid_alive(*pid)) {
        if announce {
            crate::context::say(msg::archive::waiting(session_id, pid));
        }
        while pid_alive(pid) {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if !current(ctx)? {
                // Resumed, dropped, or closed again.
                return Ok(());
            }
        }
    }

    // The work happens without the lock: a transcript can be large.
    let result = (|| -> Result<(PathBuf, String)> {
        let adapter = prov_adapters::by_agent_name(&session.agent)
            .with_context(|| msg::archive::no_adapter(&session.agent))?;
        let entries = adapter.archive_entries(&session.id, session.transcript_path.as_deref())?;
        let tar = build_tar(entries)?;
        let hash = hex(&Sha256::digest(&tar));
        let config = ctx.repo.config_here()?;
        let dir = ctx.repo.base_dir().join(&config.logs_dir).join(&session.agent);
        let file = dir.join(format!("{}-{}.tar.gz", session.id, &hash[..12]));
        remove_stale_temps(&dir, &session.id);
        write_gzip(&dir, &file, &tar, &hash)?;
        Ok((file, hash))
    })();

    let lock = ctx.store.lock()?;
    // The record can be gone (dropped), Open again (resumed), or Closing in
    // a later generation (resumed and ended again) by now.
    let Some(mut current) = lock.get(session_id)? else { return Ok(()) };
    if current.state != State::Closing || current.generation != session.generation {
        return Ok(());
    }
    match result {
        Ok((file, hash)) => current.archived(file, hash),
        Err(e) => current.failed(format!("{e:#}")),
    }
    lock.put(&current)
}

/// Entries sorted by name; modification time 0, owner 0 and mode 0644. The
/// same files always give the same bytes.
pub fn build_tar(mut entries: Vec<ArchiveEntry>) -> Result<Vec<u8>> {
    entries.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    let mut builder = tar::Builder::new(Vec::new());
    builder.mode(tar::HeaderMode::Deterministic);
    for entry in &entries {
        // Read the whole file first: the size in the header must match, and a
        // transcript can still grow.
        let data = fs::read(&entry.source).with_context(|| msg::fs::cannot_read(entry.source.display()))?;
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        builder
            .append_data(&mut header, &entry.name, data.as_slice())
            .with_context(|| msg::archive::cannot_add(&entry.name))?;
    }
    builder.into_inner().context(msg::archive::CANNOT_FINISH)
}

/// Writes the gzip file. An archive file is never overwritten. Its name
/// holds the hash of its content: a file that exists must be that archive,
/// and the step fails if it is not.
/// The process ID in the name of a temporary archive file of a session:
/// `<session>-<12 hex>.tar.tmp<pid>`, which [`write_gzip`] writes before the
/// final name.
pub fn temp_archive_pid(name: &str, session_id: &str) -> Option<u32> {
    let rest = name.strip_prefix(session_id)?.strip_prefix('-')?;
    let (hash, pid) = rest.split_once(".tar.tmp")?;
    if hash.len() != 12 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    pid.parse().ok()
}

/// Deletes the temporary archive files of a session whose process no longer
/// runs: what an archive step left that was killed during its write.
/// Reason: such a file holds part of the transcript, and nothing else
/// deletes it. Failures are ignored; the next step tries again.
fn remove_stale_temps(dir: &Path, session_id: &str) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| temp_archive_pid(n, session_id)) else { continue };
        if !pid_alive(pid) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn write_gzip(dir: &Path, file: &Path, tar: &[u8], hash: &str) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| msg::fs::cannot_create(dir.display()))?;
    if file.exists() {
        return verify_gzip(file, hash);
    }
    // A temporary file first: a process that stops during the write leaves
    // no file with the final name.
    let tmp = file.with_extension(format!("tmp{}", std::process::id()));
    let out = File::create(&tmp).with_context(|| msg::fs::cannot_write(tmp.display()))?;
    let mut gz = GzBuilder::new().mtime(0).write(out, Compression::default());
    gz.write_all(tar)?;
    gz.finish()?.sync_all()?;
    // A hard link fails if the name exists, so a concurrent archive process
    // cannot make this one overwrite a file.
    let linked = fs::hard_link(&tmp, file);
    let _ = fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => verify_gzip(file, hash),
        Err(e) => Err(e).with_context(|| msg::fs::cannot_write(file.display())),
    }
}

/// Checks that the decompressed content of an archive file has this hash.
fn verify_gzip(file: &Path, hash: &str) -> Result<()> {
    use std::io::Read;
    let mut tar = Vec::new();
    let read = File::open(file).and_then(|f| flate2::read::GzDecoder::new(f).read_to_end(&mut tar));
    if read.is_ok() && hex(&Sha256::digest(&tar)) == hash {
        return Ok(());
    }
    bail!(msg::archive::other_file(file.display(), hash))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tar_is_deterministic_and_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::write(&a, "first\n").unwrap();
        fs::write(&b, "second\n").unwrap();
        let long = format!("s/tool-results/{}.txt", "x".repeat(150));
        let entries = |order: bool| {
            let mut v = vec![
                ArchiveEntry { name: "transcript.jsonl".into(), source: a.clone() },
                ArchiveEntry { name: long.clone(), source: b.clone() },
            ];
            if order {
                v.reverse();
            }
            v
        };
        let one = build_tar(entries(false)).unwrap();
        // A later modification time and a different order change nothing.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
        File::options().write(true).open(&a).unwrap().set_modified(later).unwrap();
        let two = build_tar(entries(true)).unwrap();
        assert_eq!(one, two);

        let mut archive = tar::Archive::new(one.as_slice());
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                assert_eq!((e.header().mtime().unwrap(), e.header().uid().unwrap(), e.header().mode().unwrap()), (0, 0, 0o644));
                e.path().unwrap().to_string_lossy().into_owned()
            })
            .collect();
        assert_eq!(names, [long, "transcript.jsonl".to_string()]);
    }

    #[test]
    fn gzip_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("logs/claude/s-abc.tar.gz");
        let hash = |tar: &[u8]| hex(&Sha256::digest(tar));
        // What a process leaves that was killed during the write: a
        // temporary file, and no file with the final name.
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file.with_extension("tmp999"), b"half of a gz").unwrap();
        write_gzip(file.parent().unwrap(), &file, b"one", &hash(b"one")).unwrap();
        let first = fs::read(&file).unwrap();
        // The same archive again: the step succeeds and changes nothing.
        write_gzip(file.parent().unwrap(), &file, b"one", &hash(b"one")).unwrap();
        assert_eq!(fs::read(&file).unwrap(), first);
        // Another archive under that name: the step fails and changes nothing.
        let err = write_gzip(file.parent().unwrap(), &file, b"two", &hash(b"two")).unwrap_err();
        assert!(err.to_string().contains("never overwrites"), "{err}");
        assert_eq!(fs::read(&file).unwrap(), first);
        // A file that is not a gzip file at all.
        fs::write(&file, b"cut off").unwrap();
        assert!(write_gzip(file.parent().unwrap(), &file, b"one", &hash(b"one")).is_err());
        assert_eq!(fs::read(&file).unwrap(), b"cut off");
    }
}
