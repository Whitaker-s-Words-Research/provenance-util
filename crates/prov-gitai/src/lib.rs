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
use prov_messages as msg;
use serde::Deserialize;

/// The compatibility baseline: the oldest git-ai that `prov` supports.
pub const BASELINE_VERSION: &str = "1.7.6";
/// The newest git-ai that the contract tests (`crates/prov/tests/gitai_contract.rs`)
/// passed against. Raise it only after that suite passes against the new
/// version, and record the run in "Appendix: test evidence" of the spec.
pub const TESTED_VERSION: &str = "1.7.6";

/// How an installed git-ai version relates to the supported range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compat {
    /// Older than the baseline, or not a version that `prov` can read.
    Unsupported,
    /// From the baseline to the tested version.
    Tested,
    /// Newer than the tested version. It can work; no test shows that it does.
    Untested,
}

/// A version such as `1.7.6`, `v1.10.0-beta` or `1.7.6+abc`, as numbers.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim_start_matches('v').split(['.', '-', '+']);
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// Where `version` falls in the supported range.
pub fn compat(version: &str) -> Compat {
    let (Some(v), Some(base), Some(tested)) =
        (parse_version(version), parse_version(BASELINE_VERSION), parse_version(TESTED_VERSION))
    else {
        return Compat::Unsupported;
    };
    if v < base {
        Compat::Unsupported
    } else if v <= tested {
        Compat::Tested
    } else {
        Compat::Untested
    }
}
/// The tool name of the checkpoint that taints changes: lines that `prov`
/// cannot give to a session. Each taint gets a new conversation ID, so no
/// record and no trailer ever names it, and the gate lists its lines.
pub const UNATTRIBUTED_TOOL: &str = "prov-unattributed";
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
    let raw: RawDiff = serde_json::from_str(json).context(msg::gitai::UNEXPECTED_DIFF)?;
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

/// Parses the output of `git-ai blame <file> --json`: `lines` maps line
/// ranges (`"3-4"`, `"7"`, or several with commas) to a key of `prompts`.
pub fn parse_blame(json: &str) -> Result<Vec<(u32, u32, AiSession)>> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        lines: BTreeMap<String, String>,
        #[serde(default)]
        prompts: BTreeMap<String, RawSession>,
    }
    let raw: Raw = serde_json::from_str(json).context(msg::gitai::UNEXPECTED_BLAME)?;
    let mut out = Vec::new();
    for (ranges, key) in &raw.lines {
        let prompt = raw.prompts.get(key).with_context(|| msg::gitai::undefined_prompt(key))?;
        let a = &prompt.agent_id;
        let session =
            AiSession { tool: a.tool.clone(), id: a.id.clone(), model: a.model.clone().unwrap_or_else(|| "unknown".into()) };
        for part in ranges.split(',') {
            let part = part.trim();
            let (start, end) = match part.split_once('-') {
                Some((a, b)) => (a.parse::<u32>(), b.parse::<u32>()),
                None => (part.parse::<u32>(), part.parse::<u32>()),
            };
            let (Ok(start), Ok(end)) = (start, end) else { bail!(msg::gitai::blame_range(part)) };
            out.push((start, end, session.clone()));
        }
    }
    out.sort();
    Ok(out)
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
                serde_json::from_slice::<Status>(&out.stdout).is_ok_and(|s| s.ok)
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
            .context(msg::run::CANNOT_RUN_GITAI)?;
        child.stdin.take().context(msg::gitai::NO_STDIN)?.write_all(payload.to_string().as_bytes())?;
        let out = child.wait_with_output()?;
        if !out.status.success() {
            bail!(msg::gitai::checkpoint_failed(String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(())
    }

    /// The checkpoint before an AI edit: `agent-v1` with type `human` on
    /// `files`. What the files hold now is not the work of the session.
    pub fn checkpoint_before_ai(&self, worktree: &Path, files: &[PathBuf]) -> Result<()> {
        let payload = serde_json::json!({
            "type": "human",
            "repo_working_dir": worktree,
            "will_edit_filepaths": files,
        });
        let mut child = self
            .command(worktree)
            .args(["checkpoint", "agent-v1", "--hook-input", "stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context(msg::run::CANNOT_RUN_GITAI)?;
        child.stdin.take().context(msg::gitai::NO_STDIN)?.write_all(payload.to_string().as_bytes())?;
        let out = child.wait_with_output()?;
        if !out.status.success() {
            bail!(msg::gitai::checkpoint_failed(String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(())
    }

    /// The AI ranges of a file as it is in `worktree`, from
    /// `git-ai blame <file> --json`. Lines of humans are not listed. The
    /// file must be equal to its committed content.
    pub fn blame(&self, worktree: &Path, file: &str) -> Result<Vec<(u32, u32, AiSession)>> {
        let out = self
            .command(worktree)
            .args(["blame", file, "--json"])
            .stdin(Stdio::null())
            .output()
            .context(msg::run::CANNOT_RUN_GITAI)?;
        if !out.status.success() {
            bail!(msg::gitai::blame_failed(file, String::from_utf8_lossy(&out.stderr).trim()));
        }
        parse_blame(&String::from_utf8_lossy(&out.stdout)).with_context(|| msg::gitai::in_file(file))
    }

    /// The checkpoint of `prov approve --human`: every change to `files`
    /// since the last checkpoint belongs to the human.
    pub fn checkpoint_known_human(&self, worktree: &Path, files: &[PathBuf], editor_version: &str) -> Result<()> {
        let out = self
            .command(worktree)
            .args(["checkpoint", "known_human", "--editor", "prov", "--editor-version", editor_version, "--"])
            .args(files)
            .stdin(Stdio::null())
            .output()
            .context(msg::run::CANNOT_RUN_GITAI)?;
        if !out.status.success() {
            bail!(msg::gitai::checkpoint_failed(String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(())
    }

    /// The version that `git-ai --version` prints, for example `1.7.6`.
    pub fn version(&self, dir: &Path) -> Result<String> {
        let out = self.command(dir).arg("--version").stdin(Stdio::null()).output().context(msg::run::CANNOT_RUN_GITAI)?;
        if !out.status.success() {
            bail!(msg::gitai::VERSION_FAILED);
        }
        let text = String::from_utf8_lossy(&out.stdout);
        text.split_whitespace()
            .find(|word| word.chars().next().is_some_and(|c| c.is_ascii_digit()))
            .map(String::from)
            .context(msg::gitai::NO_VERSION)
    }

    /// A list from `git-ai config <key>`, for example `allow_repositories`.
    pub fn config_list(&self, dir: &Path, key: &str) -> Result<Vec<String>> {
        let out = self.command(dir).args(["config", key]).stdin(Stdio::null()).output().context(msg::run::CANNOT_RUN_GITAI)?;
        if !out.status.success() {
            bail!(msg::gitai::config_failed(key, String::from_utf8_lossy(&out.stderr).trim()));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let text = text.trim();
        if text.is_empty() || text == "null" {
            return Ok(Vec::new());
        }
        serde_json::from_str(text).with_context(|| msg::gitai::unexpected_config(key))
    }

    /// `git-ai debug`: the diagnostics of git-ai itself. Returns its output
    /// when it fails.
    pub fn debug(&self, dir: &Path) -> std::result::Result<(), String> {
        match self.command(dir).arg("debug").stdin(Stdio::null()).output() {
            Ok(out) if out.status.success() => Ok(()),
            Ok(out) => Err(format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout).trim(),
                String::from_utf8_lossy(&out.stderr).trim()
            )),
            Err(e) => Err(msg::gitai::cannot_run(e)),
        }
    }

    /// Waits until the service has done its queued work: the checkpoints it
    /// has received, and the notes of new commits.
    pub fn await_notes(&self, worktree: &Path) -> Result<()> {
        self.await_for(worktree, AWAIT_TIMEOUT_SECS)
    }

    /// `git-ai await` with a time limit of `secs`: the service applies the
    /// checkpoints that it has received and writes pending notes.
    pub fn await_for(&self, worktree: &Path, secs: u32) -> Result<()> {
        let out = self
            .command(worktree)
            .args(["await", "--timeout", &secs.to_string()])
            .stdin(Stdio::null())
            .output()
            .context(msg::run::CANNOT_RUN_GITAI)?;
        if !out.status.success() {
            bail!(msg::gitai::await_timed_out(secs, String::from_utf8_lossy(&out.stderr).trim()));
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
            .context(msg::run::CANNOT_RUN_GITAI)?;
        if !out.status.success() {
            bail!(msg::gitai::diff_failed(commit, String::from_utf8_lossy(&out.stderr).trim()));
        }
        parse_diff(&String::from_utf8_lossy(&out.stdout)).with_context(|| msg::gitai::in_commit(commit))
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
    fn parses_blame() {
        // Output of git-ai 1.7.6, shortened.
        let json = r#"{"lines": {"3-4": "s_e2::t_64", "7,9-10": "s_e2::t_64"},
          "prompts": {"s_e2::t_64": {"agent_id": {"tool": "claude", "id": "s1", "model": "m1"}, "human_author": "T"}},
          "metadata": {"is_logged_in": false}}"#;
        let ranges = parse_blame(json).unwrap();
        let s = AiSession { tool: "claude".into(), id: "s1".into(), model: "m1".into() };
        assert_eq!(ranges, vec![(3, 4, s.clone()), (7, 7, s.clone()), (9, 10, s)]);
        assert_eq!(parse_blame(r#"{"lines": {}, "prompts": {}}"#).unwrap(), [] as [(u32, u32, AiSession); 0]);
        assert!(parse_blame(r#"{"lines": {"1": "x"}, "prompts": {}}"#).is_err());
    }

    #[test]
    fn empty_output() {
        let a = parse_diff(r#"{"files":{},"prompts":{},"hunks":[],"commits":{}}"#).unwrap();
        assert!(a.added.is_empty() && a.sessions.is_empty());
        assert!(parse_diff("not json").is_err());
    }

    #[test]
    fn versions_and_the_supported_range() {
        assert_eq!(parse_version("1.7.6"), Some((1, 7, 6)));
        assert_eq!(parse_version("v1.10.0-beta"), Some((1, 10, 0)));
        assert_eq!(parse_version("1.7"), None);
        assert_eq!(compat("1.7"), Compat::Unsupported);
        assert_eq!(compat("1.7.5"), Compat::Unsupported);
        assert_eq!(compat(BASELINE_VERSION), Compat::Tested);
        assert_eq!(compat(TESTED_VERSION), Compat::Tested);
        let (a, b, c) = parse_version(TESTED_VERSION).unwrap();
        assert_eq!(compat(&format!("{a}.{b}.{}", c + 1)), Compat::Untested);
        assert_eq!(compat(&format!("{}.0.0", a + 1)), Compat::Untested);
    }
}
