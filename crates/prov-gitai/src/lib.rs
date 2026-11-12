//! git-ai commands and their JSON output.
//!
//! `prov` never writes git-ai data. It sets authorship only through public
//! git-ai checkpoints and reads it only through git-ai's JSON output. It has
//! no parser for the note format.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// The compatibility baseline.
pub const BASELINE_VERSION: &str = "1.7.6";
/// The time limit for `git-ai await`, in seconds.
pub const AWAIT_TIMEOUT_SECS: u32 = 60;

#[derive(Debug, Clone)]
pub struct GitAi {
    program: OsString,
}

impl Default for GitAi {
    fn default() -> Self {
        Self { program: std::env::var_os("PROV_GIT_AI").unwrap_or_else(|| "git-ai".into()) }
    }
}

/// An AI session as git-ai records it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AiSession {
    /// git-ai's name for the tool, for example `claude`.
    pub tool: String,
    /// The conversation ID.
    pub id: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Author {
    Ai(AiSession),
    Human,
    /// No range gives the line a session or a human.
    Untracked,
}

/// A range of added lines of one file in one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedRange {
    pub path: String,
    pub start: u32,
    pub end: u32,
    pub author: Author,
}

impl AddedRange {
    pub fn lines(&self) -> u32 {
        self.end.saturating_sub(self.start) + 1
    }
}

/// The authorship of one commit, from `git-ai diff <commit> --json`.
#[derive(Debug, Clone, Default)]
pub struct CommitAuthorship {
    pub added: Vec<AddedRange>,
    /// Every AI session that the note of the commit names.
    pub sessions: Vec<AiSession>,
}

#[derive(Deserialize)]
struct RawDiff {
    #[serde(default)]
    hunks: Vec<RawHunk>,
    #[serde(default)]
    sessions: BTreeMap<String, RawSession>,
}

#[derive(Deserialize)]
struct RawHunk {
    hunk_kind: String,
    start_line: u32,
    end_line: u32,
    file_path: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    human_id: Option<String>,
}

#[derive(Deserialize)]
struct RawSession {
    agent_id: RawAgentId,
}

#[derive(Deserialize)]
struct RawAgentId {
    tool: String,
    id: String,
    #[serde(default)]
    model: Option<String>,
}

/// Parses the output of `git-ai diff <commit> --json`.
///
/// A range is AI only if it has a `session_id` that the output defines. A
/// range with a `human_id` is human. Every other added range is untracked;
/// this includes the ranges of a commit with no note.
pub fn parse_diff(json: &str) -> Result<CommitAuthorship> {
    let raw: RawDiff = serde_json::from_str(json).context("unexpected output of git-ai diff --json")?;
    let sessions: BTreeMap<String, AiSession> = raw
        .sessions
        .into_iter()
        .map(|(key, s)| {
            let model = s.agent_id.model.unwrap_or_else(|| "unknown".into());
            (key, AiSession { tool: s.agent_id.tool, id: s.agent_id.id, model })
        })
        .collect();
    let added = raw
        .hunks
        .into_iter()
        .filter(|h| h.hunk_kind == "addition")
        .map(|h| {
            let author = match (h.session_id.as_ref().and_then(|id| sessions.get(id)), &h.human_id) {
                (Some(session), _) => Author::Ai(session.clone()),
                (None, Some(_)) => Author::Human,
                (None, None) => Author::Untracked,
            };
            AddedRange { path: h.file_path, start: h.start_line, end: h.end_line, author }
        })
        .collect();
    let mut sessions: Vec<AiSession> = sessions.into_values().collect();
    sessions.sort();
    sessions.dedup();
    Ok(CommitAuthorship { added, sessions })
}

impl GitAi {
    /// git-ai gets no git hook environment: it must find the repository from
    /// the directory, as it does when an agent runs it.
    fn command(&self, dir: &Path) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.current_dir(dir);
        for var in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_PREFIX", "GIT_COMMON_DIR"] {
            cmd.env_remove(var);
        }
        cmd
    }

    /// True if the git-ai service is available for this worktree. When it is
    /// not, a checkpoint exits 0 and records nothing.
    pub fn service_available(&self, worktree: &Path) -> bool {
        #[derive(Deserialize)]
        struct Status {
            ok: bool,
        }
        let out = self
            .command(worktree)
            .args(["bg", "status", "--repo"])
            .arg(worktree)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        match out {
            Ok(out) if out.status.success() => {
                serde_json::from_slice::<Status>(&out.stdout).map(|s| s.ok).unwrap_or(false)
            }
            _ => false,
        }
    }

    /// Starts the service and does not wait for it.
    pub fn start_service(&self, worktree: &Path) {
        let _ = self
            .command(worktree)
            .args(["bg", "start"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }

    /// The sweep checkpoint: `agent-v1` with type `ai_agent` on `files`.
    pub fn checkpoint_ai(
        &self,
        worktree: &Path,
        files: &[PathBuf],
        tool: &str,
        model: &str,
        conversation_id: &str,
    ) -> Result<()> {
        let payload = serde_json::json!({
            "type": "ai_agent",
            "repo_working_dir": worktree,
            "edited_filepaths": files,
            "agent_name": tool,
            "model": model,
            "conversation_id": conversation_id,
        });
        let mut child = self
            .command(worktree)
            .args(["checkpoint", "agent-v1", "--hook-input", "stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("cannot run git-ai")?;
        child.stdin.take().context("no stdin")?.write_all(payload.to_string().as_bytes())?;
        let out = child.wait_with_output()?;
        if !out.status.success() {
            bail!("git-ai checkpoint failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }

    /// Waits until the service has written the notes of new commits.
    pub fn await_notes(&self, worktree: &Path) -> Result<()> {
        let out = self
            .command(worktree)
            .args(["await", "--timeout", &AWAIT_TIMEOUT_SECS.to_string()])
            .stdin(Stdio::null())
            .output()
            .context("cannot run git-ai")?;
        if !out.status.success() {
            bail!(
                "git-ai await did not finish in {AWAIT_TIMEOUT_SECS} seconds: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    /// The authorship of one commit. This reads notes and needs no service.
    pub fn diff(&self, worktree: &Path, commit: &str) -> Result<CommitAuthorship> {
        let out = self
            .command(worktree)
            .args(["diff", commit, "--json"])
            .stdin(Stdio::null())
            .output()
            .context("cannot run git-ai")?;
        if !out.status.success() {
            bail!("git-ai diff {commit} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        parse_diff(&String::from_utf8_lossy(&out.stdout)).with_context(|| format!("commit {commit}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Output of git-ai 1.7.6 for a commit with AI, human and plain lines,
    // without the `files` and `commits` keys.
    const SAMPLE: &str = r#"{
      "prompts": {},
      "sessions": {"s_9bac45469b7c75": {"agent_id": {"tool": "claude", "id": "sess-x", "model": "m2"},
                   "human_author": "Test Human <human@example.invalid>"}},
      "humans": {"h_0f63e8dd144c42": {"author": "Test Human <human@example.invalid>"}},
      "hunks": [
        {"commit_sha": "5b", "content_hash": "6c", "hunk_kind": "deletion", "start_line": 1, "end_line": 1, "file_path": "ai.txt"},
        {"commit_sha": "5b", "content_hash": "6c", "hunk_kind": "addition", "start_line": 1, "end_line": 2, "file_path": "ai.txt",
         "prompt_id": "s_9bac45469b7c75::t_1799a30496009a", "session_id": "s_9bac45469b7c75"},
        {"commit_sha": "5b", "content_hash": "6a", "hunk_kind": "addition", "start_line": 1, "end_line": 1, "file_path": "kh.txt",
         "human_id": "h_0f63e8dd144c42"},
        {"commit_sha": "5b", "content_hash": "f6", "hunk_kind": "addition", "start_line": 3, "end_line": 5, "file_path": "plain.txt"},
        {"commit_sha": "5b", "content_hash": "f6", "hunk_kind": "addition", "start_line": 1, "end_line": 1, "file_path": "nonote.txt",
         "prompt_id": "0cd5f79210d45b9c"}
      ]
    }"#;

    #[test]
    fn parses_ai_human_and_untracked_ranges() {
        let a = parse_diff(SAMPLE).unwrap();
        let session = AiSession { tool: "claude".into(), id: "sess-x".into(), model: "m2".into() };
        assert_eq!(a.sessions, vec![session.clone()]);
        assert_eq!(a.added.len(), 4);
        assert_eq!(a.added[0].author, Author::Ai(session));
        assert_eq!(a.added[1].author, Author::Human);
        assert_eq!((a.added[2].author.clone(), a.added[2].lines()), (Author::Untracked, 3));
        // A `prompt_id` with no session is what a commit with no note gives.
        assert_eq!(a.added[3].author, Author::Untracked);
    }

    #[test]
    fn empty_output() {
        let a = parse_diff(r#"{"files":{},"prompts":{},"hunks":[],"commits":{}}"#).unwrap();
        assert!(a.added.is_empty() && a.sessions.is_empty());
        assert!(parse_diff("not json").is_err());
    }
}
