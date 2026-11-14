//! Processes that run in a worktree: the warning of R-SWEEP-4.
//!
//! A process that continues after a session ends, for example a development
//! server, is not swept. The sweep of the next session in the worktree takes
//! its changes as the work of that session. `prov` cannot tell which process
//! wrote a file, so it names the processes whose current directory is in the
//! worktree, and the person decides.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use prov_messages as msg;

use crate::context::say;

/// Shells are left out: a person's terminal in the worktree is common, and a
/// shell writes no files by itself. A job that a shell started is listed.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "fish", "dash", "ksh", "tcsh", "csh"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub command: String,
}

/// Every process with its parent and its command name, from `ps`.
fn table() -> HashMap<u32, (u32, String)> {
    let Ok(out) = Command::new("ps").args(["-A", "-o", "pid=,ppid=,comm="]).stdin(Stdio::null()).output() else {
        return HashMap::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let parent = parts.next()?.parse().ok()?;
            let comm = parts.collect::<Vec<_>>().join(" ");
            Some((pid, (parent, comm)))
        })
        .collect()
}

/// The current directory of every process that this user can see.
#[cfg(target_os = "linux")]
fn cwds() -> Vec<(u32, PathBuf)> {
    let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
    entries
        .filter_map(|e| {
            let e = e.ok()?;
            let pid = e.file_name().to_str()?.parse().ok()?;
            Some((pid, std::fs::read_link(e.path().join("cwd")).ok()?))
        })
        .collect()
}

/// The current directory of every process that this user can see.
#[cfg(not(target_os = "linux"))]
fn cwds() -> Vec<(u32, PathBuf)> {
    // `-F pn`: a line `p<pid>`, then a line `n<path>` for its cwd.
    let Ok(out) = Command::new("lsof").args(["-n", "-P", "-d", "cwd", "-F", "pn"]).stdin(Stdio::null()).output() else {
        return Vec::new();
    };
    let mut result = Vec::new();
    let mut pid = None;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().ok();
        } else if let (Some(path), Some(p)) = (line.strip_prefix('n'), pid) {
            result.push((p, PathBuf::from(path)));
        }
    }
    result
}

/// The pids whose current directory is in `root`.
fn pids_in(root: &Path) -> Vec<u32> {
    cwds().into_iter().filter(|(_, cwd)| cwd.canonicalize().is_ok_and(|c| c.starts_with(root))).map(|(pid, _)| pid).collect()
}

/// The processes whose current directory is in `worktree`, except this
/// process, its ancestors and shells.
///
/// A process counts only if it is there in two scans 300 ms apart, and
/// `ps` lists it: git and git-ai start short processes in a worktree, and
/// these are not what R-SWEEP-4 is about.
pub fn in_worktree(worktree: &Path) -> Vec<Proc> {
    let Ok(root) = worktree.canonicalize() else { return Vec::new() };
    let first = pids_in(&root);
    if first.is_empty() {
        return Vec::new();
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    let second = pids_in(&root);
    let table = table();
    let mut skip = vec![std::process::id()];
    let mut pid = std::process::id();
    while let Some((parent, _)) = table.get(&pid) {
        if *parent == 0 || skip.contains(parent) {
            break;
        }
        skip.push(*parent);
        pid = *parent;
    }
    let mut found: Vec<Proc> = second
        .into_iter()
        .filter(|pid| first.contains(pid) && !skip.contains(pid))
        .filter_map(|pid| {
            let (_, comm) = table.get(&pid)?;
            let name = Path::new(comm.trim_start_matches('-')).file_name()?.to_str()?;
            (!SHELLS.contains(&name)).then(|| Proc { pid, command: comm.clone() })
        })
        .collect();
    found.sort_by_key(|p| p.pid);
    found.dedup();
    found
}

/// Prints the warning of R-SWEEP-4 if processes run in `worktree`. `then`
/// says what happens next, for example "Stop them before another session
/// starts here."
pub fn warn_if_busy(worktree: &Path, then: &str) {
    let procs = in_worktree(worktree);
    if procs.is_empty() {
        return;
    }
    let mut text = format!("{}\n", msg::procs::busy_header(worktree.display()));
    for p in &procs {
        let _ = writeln!(text, "{}", msg::procs::busy_process(p.pid, &p.command));
    }
    text.push_str(msg::procs::BUSY_RISK);
    text.push_str(then);
    say(text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_child_in_the_worktree_and_not_itself() {
        let dir = tempfile::tempdir().unwrap();
        let mut child = Command::new("sleep").arg("30").current_dir(dir.path()).spawn().unwrap();
        let found = in_worktree(dir.path());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(found.iter().any(|p| p.pid == child.id() && p.command.contains("sleep")), "{found:?}");
        assert!(!found.iter().any(|p| p.pid == std::process::id()), "{found:?}");
    }

    #[test]
    fn leaves_out_shells() {
        let dir = tempfile::tempdir().unwrap();
        let mut child = Command::new("sh").args(["-c", "read x"]).current_dir(dir.path()).stdin(Stdio::piped()).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let found = in_worktree(dir.path());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(found.is_empty(), "{found:?}");
    }
}
