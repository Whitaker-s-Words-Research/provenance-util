//! The archive step: a deterministic tar of the transcript, its SHA-256
//! before compression, and the gzip file in `logs_dir`.

use std::fs::{self, File};
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use flate2::{Compression, GzBuilder};
use prov_adapters::ArchiveEntry;
use prov_core::State;
use sha2::{Digest, Sha256};

use crate::context::Ctx;

/// Starts `prov archive-session <id>` in its own process session, so that it
/// continues when the agent and its hook process exit.
pub fn spawn_detached(ctx: &Ctx, session_id: &str) -> Result<u32> {
    let exe = std::env::current_exe().context("cannot find the prov binary")?;
    let mut cmd = Command::new(exe);
    cmd.args(["archive-session", session_id])
        .current_dir(&ctx.repo.toplevel)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe and touches no memory of the parent.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let child = cmd.spawn().context("cannot start the archive process")?;
    Ok(child.id())
}

/// The archive step of one Closing session, for the detached process.
pub fn run(session_id: &str) -> Result<()> {
    run_in(&Ctx::here()?, session_id)
}

/// The archive step of one Closing session. The result is in the record:
/// Archived, or Failed with the error text.
pub fn run_in(ctx: &Ctx, session_id: &str) -> Result<()> {
    let session = {
        let lock = ctx.store.lock()?;
        let Some(mut session) = lock.get(session_id)? else { bail!("no record of session {session_id}") };
        if session.state != State::Closing {
            bail!("session {session_id} is {}, not closing", session.state.as_str());
        }
        session.archive_pid = Some(std::process::id());
        lock.put(&session)?;
        session
    };

    // The work happens without the lock: a transcript can be large.
    let result = (|| -> Result<(PathBuf, String)> {
        let adapter = prov_adapters::by_agent_name(&session.agent)
            .with_context(|| format!("no adapter for the agent `{}`", session.agent))?;
        let entries = adapter.archive_entries(&session.id, session.transcript_path.as_deref())?;
        let tar = build_tar(entries)?;
        let hash = hex(&Sha256::digest(&tar));
        let config = ctx.repo.config_here()?;
        let dir = ctx.repo.base_dir().join(&config.logs_dir).join(&session.agent);
        let file = dir.join(format!("{}-{}.tar.gz", session.id, &hash[..12]));
        write_gzip(&dir, &file, &tar)?;
        Ok((file, hash))
    })();

    let lock = ctx.store.lock()?;
    // The record can be gone (dropped) or Open again (resumed) by now.
    let Some(mut current) = lock.get(session_id)? else { return Ok(()) };
    if current.state != State::Closing {
        return Ok(());
    }
    current.last_event_at = current.last_event_at.max(session.last_event_at);
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
        let data = fs::read(&entry.source).with_context(|| format!("cannot read {}", entry.source.display()))?;
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        builder
            .append_data(&mut header, &entry.name, data.as_slice())
            .with_context(|| format!("cannot add {} to the archive", entry.name))?;
    }
    builder.into_inner().context("cannot finish the archive")
}

/// Writes the gzip file. An archive file is never overwritten: its name holds
/// the hash of its content, so an existing file is the same archive.
fn write_gzip(dir: &Path, file: &Path, tar: &[u8]) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    if file.exists() {
        return Ok(());
    }
    let tmp = file.with_extension(format!("tmp{}", std::process::id()));
    let out = File::create(&tmp).with_context(|| format!("cannot write {}", tmp.display()))?;
    let mut gz = GzBuilder::new().mtime(0).write(out, Compression::default());
    gz.write_all(tar)?;
    gz.finish()?.sync_all()?;
    // A hard link fails if the name exists, so a concurrent archive process
    // cannot make this one overwrite a file.
    let linked = fs::hard_link(&tmp, file);
    let _ = fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e).with_context(|| format!("cannot write {}", file.display())),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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
        write_gzip(file.parent().unwrap(), &file, b"one").unwrap();
        let first = fs::read(&file).unwrap();
        write_gzip(file.parent().unwrap(), &file, b"two").unwrap();
        assert_eq!(fs::read(&file).unwrap(), first);
        assert_eq!(fs::read_dir(file.parent().unwrap()).unwrap().count(), 1);
    }
}
