//! Claude Code. Events: SessionStart, UserPromptSubmit, Stop, SessionEnd.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use serde_json::{json, Map, Value};

use crate::{commit_env, Adapter, AgentEvent, ArchiveEntry, EventKind, Identity};

/// The events that get a hook, in the order of the settings file.
const EVENTS: [&str; 4] = ["SessionStart", "UserPromptSubmit", "Stop", "SessionEnd"];

fn hook_command(event: &str) -> String {
    format!("prov hook claude-code {event}")
}

/// True if the entries of one event hold a hook with this command.
fn has_hook(entries: Option<&Value>, command: &str) -> bool {
    entries.and_then(Value::as_array).is_some_and(|groups| {
        groups.iter().any(|group| {
            group.get("hooks").and_then(Value::as_array).is_some_and(|hooks| {
                hooks.iter().any(|h| h.get("command").and_then(Value::as_str).map(str::trim) == Some(command))
            })
        })
    })
}

fn parse_settings(existing: Option<&str>) -> Result<Map<String, Value>> {
    match existing.map(str::trim).filter(|t| !t.is_empty()) {
        None => Ok(Map::new()),
        Some(text) => match serde_json::from_str::<Value>(text).context(".claude/settings.json does not parse")? {
            Value::Object(map) => Ok(map),
            _ => bail!(".claude/settings.json is not a JSON object"),
        },
    }
}

pub struct ClaudeCode;

/// The fields of a Claude Code hook payload that `prov` uses.
#[derive(Deserialize)]
struct Payload {
    session_id: String,
    #[serde(default)]
    transcript_path: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

/// How much of the end of a transcript is read to find the latest model.
const TAIL_BYTES: u64 = 512 * 1024;

impl Adapter for ClaudeCode {
    fn adapter_name(&self) -> &'static str {
        "claude-code"
    }

    fn agent_name(&self) -> &'static str {
        "claude"
    }

    /// git-ai recognizes this identity: a committed line with no checkpoint
    /// is then attributed to Claude, not to the committer as a known human.
    fn default_identity(&self) -> Identity {
        Identity { name: "Claude".into(), email: "noreply@anthropic.com".into() }
    }

    fn default_command(&self) -> Vec<String> {
        vec!["claude".into()]
    }

    fn env_markers(&self) -> &'static [&'static str] {
        &["CLAUDECODE", "CLAUDE_CODE_SESSION_ID"]
    }

    fn session_id_env(&self) -> Option<&'static str> {
        Some("CLAUDE_CODE_SESSION_ID")
    }

    /// Claude Code runs a hook through a shell that exits, so the parent
    /// process is not the agent. Only `CLAUDE_PID` names it.
    fn agent_pid(&self) -> Option<u32> {
        std::env::var("CLAUDE_PID").ok()?.trim().parse().ok().filter(|pid| *pid > 0)
    }

    fn parse_event(&self, event: &str, payload: &str) -> Result<AgentEvent> {
        let kind = match event {
            "SessionStart" => EventKind::Start,
            "UserPromptSubmit" => EventKind::Activity,
            "Stop" => EventKind::WorkDone,
            "SessionEnd" => EventKind::End,
            other => bail!("unknown Claude Code event `{other}`"),
        };
        let p: Payload = serde_json::from_str(payload).context("the hook payload does not parse")?;
        if p.session_id.is_empty() {
            bail!("the hook payload has no session_id");
        }
        let non_empty = |s: Option<String>| s.filter(|s| !s.is_empty());
        Ok(AgentEvent {
            kind,
            session_id: p.session_id,
            cwd: non_empty(p.cwd).map(PathBuf::from),
            transcript_path: non_empty(p.transcript_path).map(PathBuf::from),
            model: non_empty(p.model),
        })
    }

    /// At each Stop. There is no sweep at the end of a session: the agent
    /// changes nothing after its last Stop.
    fn sweeps_at(&self, kind: EventKind) -> bool {
        kind == EventKind::WorkDone
    }

    /// The model of the latest assistant message in the transcript.
    fn latest_model(&self, transcript: &Path) -> Option<String> {
        #[derive(Deserialize)]
        struct Line {
            #[serde(rename = "type")]
            kind: Option<String>,
            message: Option<Message>,
        }
        #[derive(Deserialize)]
        struct Message {
            model: Option<String>,
        }
        let mut file = File::open(transcript).ok()?;
        let len = file.metadata().ok()?.len();
        file.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES))).ok()?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).ok()?;
        let text = String::from_utf8_lossy(&bytes);
        text.lines().rev().find_map(|line| {
            // A cheap test first: most lines are not assistant messages.
            if !line.contains("\"assistant\"") {
                return None;
            }
            let parsed: Line = serde_json::from_str(line).ok()?;
            if parsed.kind.as_deref() != Some("assistant") {
                return None;
            }
            // Claude Code marks its own error messages with a model in `<...>`.
            parsed.message?.model.filter(|m| !m.is_empty() && !m.starts_with('<'))
        })
    }

    /// The transcript as `transcript.jsonl`, and every file of the directory
    /// `<session ID>/` next to it. Claude Code stores large tool results and
    /// the messages of subagents there, not in the transcript.
    fn archive_entries(&self, session_id: &str, transcript: Option<&Path>) -> Result<Vec<ArchiveEntry>> {
        let transcript = transcript.context("the session has no transcript path")?;
        if !transcript.is_file() {
            bail!("the transcript {} does not exist", transcript.display());
        }
        let mut entries = vec![ArchiveEntry { name: "transcript.jsonl".into(), source: transcript.to_path_buf() }];
        if let Some(dir) = transcript.parent().map(|p| p.join(session_id)) {
            if dir.is_dir() {
                collect(&dir, session_id, &mut entries)?;
            }
        }
        Ok(entries)
    }

    /// Claude Code adds the output of a SessionStart hook to the context. It
    /// does the same at UserPromptSubmit, which is why `prov` prints nothing there.
    fn message_reaches_agent(&self, kind: EventKind) -> bool {
        kind == EventKind::Start
    }

    fn resume_args(&self, session_id: &str) -> Vec<String> {
        vec!["--resume".into(), session_id.into()]
    }

    fn model_provider(&self) -> &'static str {
        "anthropic"
    }

    fn settings_file(&self) -> &'static str {
        ".claude/settings.json"
    }

    /// `env` gets the identity and the signing setting: every command that
    /// Claude Code runs gets them. `hooks` gets one entry for each event.
    fn merge_settings(&self, existing: Option<&str>, identity: &Identity) -> Result<String> {
        let mut root = parse_settings(existing)?;
        let env = root.entry("env").or_insert_with(|| json!({}));
        let Some(env) = env.as_object_mut() else { bail!("`env` in .claude/settings.json is not an object") };
        for (key, value) in commit_env(identity) {
            env.insert(key.to_string(), Value::String(value));
        }
        let hooks = root.entry("hooks").or_insert_with(|| json!({}));
        let Some(hooks) = hooks.as_object_mut() else { bail!("`hooks` in .claude/settings.json is not an object") };
        for event in EVENTS {
            let command = hook_command(event);
            if has_hook(hooks.get(event), &command) {
                continue;
            }
            let entries = hooks.entry(event).or_insert_with(|| json!([]));
            let Some(entries) = entries.as_array_mut() else {
                bail!("`hooks.{event}` in .claude/settings.json is not an array")
            };
            entries.push(json!({"hooks": [{"type": "command", "command": command}]}));
        }
        Ok(format!("{}\n", serde_json::to_string_pretty(&Value::Object(root))?))
    }

    fn settings_problems(&self, existing: Option<&str>, identity: &Identity) -> Vec<String> {
        let root = match (existing, parse_settings(existing)) {
            (None, _) => return vec![".claude/settings.json does not exist".into()],
            (_, Err(e)) => return vec![format!("{e:#}")],
            (_, Ok(root)) => root,
        };
        let mut problems = Vec::new();
        let env = root.get("env").and_then(Value::as_object);
        for (key, value) in commit_env(identity) {
            let found = env.and_then(|e| e.get(key)).and_then(Value::as_str);
            if found != Some(value.as_str()) {
                problems.push(format!("`env.{key}` is {}, not `{value}`", found.map_or("not set".into(), |f| format!("`{f}`"))));
            }
        }
        for event in EVENTS {
            if !has_hook(root.get("hooks").and_then(|h| h.get(event)), &hook_command(event)) {
                problems.push(format!("no hook `{}` for the event {event}", hook_command(event)));
            }
        }
        problems
    }

    /// `git-ai install-hooks` writes `git-ai checkpoint` hooks for the tool
    /// events into the user's Claude Code settings.
    fn gitai_integration(&self, home: &Path) -> std::result::Result<(), String> {
        let path = home.join(".claude/settings.json");
        let text = std::fs::read_to_string(&path).map_err(|_| format!("{} does not exist", path.display()))?;
        let root: Value = serde_json::from_str(&text).map_err(|e| format!("{} does not parse: {e}", path.display()))?;
        for event in ["PreToolUse", "PostToolUse"] {
            let found = root.get("hooks").and_then(|h| h.get(event)).and_then(Value::as_array).is_some_and(|groups| {
                groups.iter().any(|group| {
                    group.get("hooks").and_then(Value::as_array).is_some_and(|hooks| {
                        hooks.iter().any(|h| {
                            h.get("command").and_then(Value::as_str).is_some_and(|c| c.contains("git-ai") && c.contains("checkpoint"))
                        })
                    })
                })
            });
            if !found {
                return Err(format!("{} has no `git-ai checkpoint` hook for {event}. Run: git-ai install-hooks", path.display()));
            }
        }
        Ok(())
    }
}

fn collect(dir: &Path, prefix: &str, out: &mut Vec<ArchiveEntry>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { bail!("a file name in {} is not UTF-8", dir.display()) };
        let name = format!("{prefix}/{name}");
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect(&entry.path(), &name, out)?;
        } else if kind.is_file() {
            out.push(ArchiveEntry { name, source: entry.path() });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_recorded_payloads() {
        let start = r#"{"session_id":"5c1e2b9a-0000-4000-8000-000000000001","transcript_path":"/h/.claude/projects/p/5c1e.jsonl",
            "cwd":"/w/repo","hook_event_name":"SessionStart","source":"startup","model":"claude-fable-5-2"}"#;
        let e = ClaudeCode.parse_event("SessionStart", start).unwrap();
        assert_eq!(e.kind, EventKind::Start);
        assert_eq!(e.model.as_deref(), Some("claude-fable-5-2"));
        assert_eq!(e.cwd.as_deref(), Some(Path::new("/w/repo")));

        let stop = r#"{"session_id":"s","transcript_path":"","cwd":"/w","hook_event_name":"Stop","stop_hook_active":false}"#;
        let e = ClaudeCode.parse_event("Stop", stop).unwrap();
        assert_eq!((e.kind, e.transcript_path, e.model), (EventKind::WorkDone, None, None));

        assert!(ClaudeCode.parse_event("PreToolUse", stop).is_err());
        assert!(ClaudeCode.parse_event("Stop", "{}").is_err());
    }

    #[test]
    fn latest_model_is_the_last_assistant_message() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"user","message":{"role":"user","content":"hi"}}"#, "\n",
                r#"{"type":"assistant","message":{"role":"assistant","model":"claude-opus-5-5","content":[]}}"#, "\n",
                r#"{"type":"assistant","message":{"role":"assistant","model":"claude-fable-5-2","content":[]}}"#, "\n",
                r#"{"type":"assistant","message":{"role":"assistant","model":"<synthetic>","content":[]}}"#, "\n",
                r#"{"type":"user","message":{"role":"user","content":"the \"assistant\" said"}}"#, "\n",
            ),
        )
        .unwrap();
        assert_eq!(ClaudeCode.latest_model(&path).as_deref(), Some("claude-fable-5-2"));
        assert_eq!(ClaudeCode.latest_model(&dir.path().join("none.jsonl")), None);
    }

    #[test]
    fn archive_has_the_transcript_and_the_session_directory() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("abc.jsonl");
        std::fs::write(&transcript, "{}\n").unwrap();
        std::fs::create_dir_all(dir.path().join("abc/subagents")).unwrap();
        std::fs::create_dir_all(dir.path().join("abc/tool-results")).unwrap();
        std::fs::write(dir.path().join("abc/subagents/agent-1.jsonl"), "{}\n").unwrap();
        std::fs::write(dir.path().join("abc/tool-results/r1.txt"), "big").unwrap();
        std::fs::write(dir.path().join("other.jsonl"), "{}\n").unwrap();
        let mut names: Vec<String> =
            ClaudeCode.archive_entries("abc", Some(&transcript)).unwrap().into_iter().map(|e| e.name).collect();
        names.sort();
        assert_eq!(names, ["abc/subagents/agent-1.jsonl", "abc/tool-results/r1.txt", "transcript.jsonl"]);
        assert!(ClaudeCode.archive_entries("abc", None).is_err());
        assert!(ClaudeCode.archive_entries("abc", Some(&dir.path().join("missing.jsonl"))).is_err());
    }

    #[test]
    fn settings_merge_keeps_other_keys_and_is_stable() {
        let identity = ClaudeCode.default_identity();
        let existing = r#"{"model": "opus", "env": {"FOO": "1"}, "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "other"}]}]}}"#;
        assert!(!ClaudeCode.settings_problems(Some(existing), &identity).is_empty());
        let merged = ClaudeCode.merge_settings(Some(existing), &identity).unwrap();
        let v: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(v["model"], "opus");
        assert_eq!(v["env"]["FOO"], "1");
        assert_eq!(v["env"]["GIT_AUTHOR_EMAIL"], "noreply@anthropic.com");
        assert_eq!(v["env"]["GIT_CONFIG_KEY_0"], "commit.gpgsign");
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(v["hooks"]["SessionEnd"][0]["hooks"][0]["command"], "prov hook claude-code SessionEnd");
        assert!(ClaudeCode.settings_problems(Some(&merged), &identity).is_empty());
        // A second merge changes nothing.
        assert_eq!(ClaudeCode.merge_settings(Some(&merged), &identity).unwrap(), merged);
        // A new file.
        let new = ClaudeCode.merge_settings(None, &identity).unwrap();
        assert!(ClaudeCode.settings_problems(Some(&new), &identity).is_empty());
        assert_eq!(ClaudeCode.settings_problems(None, &identity).len(), 1);
        assert!(ClaudeCode.merge_settings(Some("[]"), &identity).is_err());
    }
}
