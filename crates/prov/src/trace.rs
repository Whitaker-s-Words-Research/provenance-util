//! `prov trace export [<range>]`: Agent Trace records as JSON Lines, one
//! record for each commit with a note.
//!
//! The schema is Agent Trace 0.1.0, pinned in `vendor/agent-trace/0.1.0/`.
//! Each record is validated against it before it is written.

use std::collections::BTreeSet;
use std::io::Write;

use anyhow::{anyhow, bail, Context, Result};
use prov_gitai::{AddedRange, Author};
use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};

use crate::context::Ctx;
use crate::schema;

const SCHEMA: &str = include_str!("../../../vendor/agent-trace/0.1.0/trace-record.schema.json");

/// The schema requires the pattern `^[0-9]+\.[0-9]+$`, which `0.1.0` does not match.
const RECORD_VERSION: &str = "0.1";

/// The namespace of the record IDs: a version 5 UUID of the commit SHA in
/// this namespace. It must never change, or the same commit gets a new ID.
const NAMESPACE: [u8; 16] =
    [0x8b, 0x1d, 0x5c, 0x3e, 0x42, 0x7a, 0x4f, 0x0e, 0x9d, 0x6b, 0x2a, 0x91, 0xc4, 0x5e, 0x7f, 0x30];

/// A version 5 UUID (SHA-1) of `name` in [`NAMESPACE`].
pub fn record_id(name: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(NAMESPACE);
    hasher.update(name.as_bytes());
    let hash = hasher.finalize();
    let mut b = [0u8; 16];
    b.copy_from_slice(&hash[..16]);
    b[6] = (b[6] & 0x0f) | 0x50;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// `<provider>/<model>`. The adapter of the tool adds the provider when the
/// note has only the model. `None` if the model is not known.
fn model_id(tool: &str, model: &str) -> Option<String> {
    if model.is_empty() || model == "unknown" {
        return None;
    }
    if model.contains('/') {
        return Some(model.to_string());
    }
    match prov_adapters::by_agent_name(tool) {
        Some(adapter) => Some(format!("{}/{model}", adapter.model_provider())),
        None => Some(model.to_string()),
    }
}

/// The contributor of a range, and a key that keeps the ranges of one author together.
fn contributor(author: &Author) -> (String, Value) {
    match author {
        Author::Ai(session) => {
            let mut c = Map::new();
            c.insert("type".into(), json!("ai"));
            if let Some(id) = model_id(&session.tool, &session.model) {
                c.insert("model_id".into(), json!(id));
            }
            (format!("ai\u{1f}{}\u{1f}{}\u{1f}{}", session.tool, session.id, session.model), Value::Object(c))
        }
        Author::Human => ("human".into(), json!({"type": "human"})),
        Author::Untracked => ("unknown".into(), json!({"type": "unknown"})),
    }
}

/// One record. `added` are the added ranges of the commit, from `git-ai diff`.
pub fn record(sha: &str, timestamp: &str, mut added: Vec<AddedRange>) -> Value {
    added.sort_by(|a, b| (&a.path, a.start, a.end).cmp(&(&b.path, b.start, b.end)));
    let mut files: Vec<Value> = Vec::new();
    let mut i = 0;
    while i < added.len() {
        let path = added[i].path.clone();
        // One conversation for each author, in the order of its first range.
        let mut conversations: Vec<(String, Value, Vec<Value>)> = Vec::new();
        while i < added.len() && added[i].path == path {
            let (key, who) = contributor(&added[i].author);
            let range = json!({"start_line": added[i].start, "end_line": added[i].end});
            match conversations.iter_mut().find(|(k, _, _)| *k == key) {
                Some((_, _, ranges)) => ranges.push(range),
                None => conversations.push((key, who, vec![range])),
            }
            i += 1;
        }
        let conversations: Vec<Value> =
            conversations.into_iter().map(|(_, who, ranges)| json!({"contributor": who, "ranges": ranges})).collect();
        files.push(json!({"path": path, "conversations": conversations}));
    }
    json!({
        "version": RECORD_VERSION,
        "id": record_id(sha),
        "timestamp": timestamp,
        "vcs": {"type": "git", "revision": sha},
        "tool": {"name": "prov", "version": prov_core::VERSION},
        "files": files,
    })
}

pub fn run(range: Option<&str>) -> Result<bool> {
    let ctx = Ctx::here()?;
    let repo = &ctx.repo;
    let schema: Value = serde_json::from_str(SCHEMA).context("the pinned Agent Trace schema does not parse")?;
    // The default range is the branch that is checked out.
    let range = range.unwrap_or("HEAD");
    if range.starts_with('-') {
        bail!("give a range, for example main..HEAD");
    }
    // Which commits have a note. The content of a note is read only through git-ai.
    let noted: BTreeSet<String> = repo
        .git_opt(["notes", "--ref=ai", "list"])?
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1).map(String::from))
        .collect();
    let log = repo.git(["log", "--reverse", "--topo-order", "--format=%H %cI", range, "--"])?;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in log.lines() {
        let Some((sha, timestamp)) = line.split_once(' ') else { continue };
        if !noted.contains(sha) {
            continue;
        }
        let authorship = ctx.gitai.diff(&repo.toplevel, sha)?;
        let record = record(sha, timestamp, authorship.added);
        schema::validate(&schema, &record)
            .map_err(|e| anyhow!("the record of commit {sha} is not valid Agent Trace 0.1.0: {e}"))?;
        writeln!(out, "{}", serde_json::to_string(&record)?)?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prov_gitai::AiSession;

    fn range(path: &str, start: u32, end: u32, author: Author) -> AddedRange {
        AddedRange { path: path.into(), start, end, author }
    }

    fn ai(id: &str, model: &str) -> Author {
        Author::Ai(AiSession { tool: "claude".into(), id: id.into(), model: model.into() })
    }

    #[test]
    fn ids_are_version_5_and_stable() {
        let id = record_id("0123456789abcdef0123456789abcdef01234567");
        assert_eq!(id, record_id("0123456789abcdef0123456789abcdef01234567"));
        assert_ne!(id, record_id("1123456789abcdef0123456789abcdef01234567"));
        assert_eq!(id.as_bytes()[14], b'5');
        assert!(matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
    }

    #[test]
    fn model_ids() {
        assert_eq!(model_id("claude", "claude-opus-4-5").as_deref(), Some("anthropic/claude-opus-4-5"));
        assert_eq!(model_id("claude", "openrouter/x").as_deref(), Some("openrouter/x"));
        assert_eq!(model_id("claude", "unknown"), None);
        assert_eq!(model_id("other-tool", "m1").as_deref(), Some("m1"));
    }

    #[test]
    fn a_record_is_valid_and_ordered() {
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        let added = vec![
            range("b.py", 1, 2, Author::Untracked),
            range("a.py", 5, 9, ai("s1", "m1")),
            range("a.py", 1, 4, Author::Human),
            range("a.py", 10, 12, ai("s1", "m1")),
            range("a.py", 13, 13, ai("s2", "unknown")),
        ];
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let one = record(sha, "2026-11-11T10:00:00+01:00", added.clone());
        schema::validate(&schema, &one).unwrap();
        assert_eq!(one["version"], "0.1");
        assert_eq!(one["files"][0]["path"], "a.py");
        let conversations = one["files"][0]["conversations"].as_array().unwrap();
        assert_eq!(conversations.len(), 3);
        assert_eq!(conversations[0]["contributor"], json!({"type": "human"}));
        assert_eq!(conversations[1]["contributor"], json!({"type": "ai", "model_id": "anthropic/m1"}));
        assert_eq!(conversations[1]["ranges"].as_array().unwrap().len(), 2);
        // The model is not known: no model_id.
        assert_eq!(conversations[2]["contributor"], json!({"type": "ai"}));
        assert_eq!(one["files"][1]["conversations"][0]["contributor"]["type"], "unknown");
        // The order of the input changes nothing.
        let mut reversed = added;
        reversed.reverse();
        assert_eq!(serde_json::to_string(&one).unwrap(), serde_json::to_string(&record(sha, "2026-11-11T10:00:00+01:00", reversed)).unwrap());
        // The schema refuses what it does not know.
        let mut bad = one.clone();
        bad["extra"] = json!(1);
        assert!(schema::validate(&schema, &bad).is_err());
        let mut bad = one;
        bad["version"] = json!("0.1.0");
        assert!(schema::validate(&schema, &bad).is_err());
    }
}
