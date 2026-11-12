//! The trailer format. Each trailer value is one line.

use std::fmt;

use anyhow::{bail, Result};

pub const PROV_SESSION: &str = "Prov-Session";
pub const PROV_TRANSCRIPT_SHA256: &str = "Prov-Transcript-SHA256";
pub const PROV_APPROVED: &str = "Prov-Approved";
pub const PROV_VERSION: &str = "Prov-Version";
pub const COPIED_FROM: &str = "Copied-From";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trailer {
    pub key: String,
    pub value: String,
}

impl Trailer {
    /// Refuses a value with a line break, and an empty value.
    pub fn new(key: &str, value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.contains(['\n', '\r']) {
            bail!("the value of a {key} trailer must be one line");
        }
        if value.trim().is_empty() {
            bail!("the value of a {key} trailer is empty");
        }
        Ok(Self { key: key.to_string(), value: value.trim().to_string() })
    }
}

impl fmt::Display for Trailer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.key, self.value)
    }
}

/// Parses `Key: value` lines, as `git interpret-trailers --parse` and
/// `%(trailers:only,unfold)` print them. Other lines are ignored.
pub fn parse_block(text: &str) -> Vec<Trailer> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            let key = key.trim();
            let ok_key = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
            (ok_key && !value.trim().is_empty())
                .then(|| Trailer { key: key.to_string(), value: value.trim().to_string() })
        })
        .collect()
}

/// The value of a `Prov-Session` trailer: `<agent>:<session ID>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionRef {
    pub agent: String,
    pub id: String,
}

impl SessionRef {
    pub fn new(agent: impl Into<String>, id: impl Into<String>) -> Self {
        Self { agent: agent.into(), id: id.into() }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let (agent, id) = value.trim().split_once(':')?;
        (!agent.is_empty() && !id.is_empty()).then(|| Self::new(agent, id))
    }
}

impl fmt::Display for SessionRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.agent, self.id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Ack,
    Human,
    Ai,
}

impl Choice {
    pub fn as_str(self) -> &'static str {
        match self {
            Choice::Ack => "ack",
            Choice::Human => "human",
            Choice::Ai => "ai",
        }
    }
}

/// The value of a `Prov-Approved` trailer: `<path> <choice>`. The choice is
/// the last word; the path can contain spaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub path: String,
    pub choice: Choice,
}

impl Approval {
    pub fn parse(value: &str) -> Option<Self> {
        let (path, choice) = value.trim().rsplit_once(' ')?;
        let choice = match choice {
            "ack" => Choice::Ack,
            "human" => Choice::Human,
            "ai" => Choice::Ai,
            _ => return None,
        };
        let path = path.trim();
        (!path.is_empty()).then(|| Self { path: path.to_string(), choice })
    }

    pub fn value(&self) -> String {
        format!("{} {}", self.path, self.choice.as_str())
    }
}

pub fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The session groups of an integration: each `Prov-Session` with the
/// `Prov-Transcript-SHA256` that directly follows it, if one does.
pub fn session_groups(trailers: &[Trailer]) -> Vec<(SessionRef, Option<String>)> {
    let mut groups = Vec::new();
    let mut iter = trailers.iter().peekable();
    while let Some(t) = iter.next() {
        if t.key != PROV_SESSION {
            continue;
        }
        let Some(session) = SessionRef::parse(&t.value) else { continue };
        let hash = match iter.peek() {
            Some(next) if next.key == PROV_TRANSCRIPT_SHA256 => iter.next().map(|n| n.value.clone()),
            _ => None,
        };
        groups.push((session, hash));
    }
    groups
}

pub fn sessions(trailers: &[Trailer]) -> Vec<SessionRef> {
    trailers.iter().filter(|t| t.key == PROV_SESSION).filter_map(|t| SessionRef::parse(&t.value)).collect()
}

pub fn approvals(trailers: &[Trailer]) -> Vec<Approval> {
    trailers.iter().filter(|t| t.key == PROV_APPROVED).filter_map(|t| Approval::parse(&t.value)).collect()
}

pub fn has(trailers: &[Trailer], key: &str) -> bool {
    trailers.iter().any(|t| t.key == key)
}

/// The path of a `Copied-From` value: `<source>@<commit>:<path>`.
pub fn copied_from_path(value: &str) -> Option<&str> {
    let bytes = value.as_bytes();
    // Find `@<40 or 64 hex>:` from the right; the source can contain `@` and `:`.
    for (at, _) in value.match_indices('@').collect::<Vec<_>>().into_iter().rev() {
        let rest = &bytes[at + 1..];
        let hex = rest.iter().take_while(|b| b.is_ascii_hexdigit()).count();
        if (hex == 40 || hex == 64) && rest.get(hex) == Some(&b':') {
            return Some(&value[at + 1 + hex + 1..]);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Property test: no value with a line break is accepted, wherever the
    /// break is. A small fixed-seed generator keeps the test deterministic.
    #[test]
    fn refuses_every_value_with_a_line_break() {
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let alphabet: Vec<char> = "abcXYZ019 :/.-_@#é\t".chars().collect();
        for _ in 0..2000 {
            let len = (next() % 40) as usize;
            let mut value: Vec<char> = (0..len).map(|_| alphabet[(next() % alphabet.len() as u64) as usize]).collect();
            let clean: String = value.iter().collect();
            if !clean.trim().is_empty() {
                assert!(Trailer::new(PROV_APPROVED, clean.clone()).is_ok(), "{clean:?}");
            }
            let pos = (next() % (len as u64 + 1)) as usize;
            let brk = match next() % 3 {
                0 => "\n",
                1 => "\r",
                _ => "\r\n",
            };
            for (i, c) in brk.chars().enumerate() {
                value.insert(pos + i, c);
            }
            let broken: String = value.iter().collect();
            for key in [PROV_SESSION, PROV_TRANSCRIPT_SHA256, PROV_APPROVED, PROV_VERSION, COPIED_FROM] {
                assert!(Trailer::new(key, broken.clone()).is_err(), "{broken:?}");
            }
        }
    }

    #[test]
    fn approval_path_can_contain_spaces() {
        let a = Approval::parse("docs/my notes.txt ack").unwrap();
        assert_eq!(a.path, "docs/my notes.txt");
        assert_eq!(a.choice, Choice::Ack);
        assert!(Approval::parse("notes.txt maybe").is_none());
        assert!(Approval::parse("ack").is_none());
    }

    #[test]
    fn groups_follow_the_spec_example() {
        let hash_a = "9f2c".repeat(16);
        let hash_b = "41b8".repeat(16);
        let text = format!(
            "Prov-Session: claude:5c1e\nProv-Transcript-SHA256: {hash_a}\nProv-Session: claude:a71d\n\
             Prov-Transcript-SHA256: {hash_b}\nProv-Approved: notes.txt ack\nProv-Version: 0.1.0+abc\n"
        );
        let trailers = parse_block(&text);
        let groups = session_groups(&trailers);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0], (SessionRef::new("claude", "5c1e"), Some(hash_a.clone())));
        assert!(is_sha256(&hash_a));
        assert!(!is_sha256("9F2C"));
        assert!(has(&trailers, PROV_VERSION));
        assert_eq!(approvals(&trailers).len(), 1);
    }

    #[test]
    fn copied_from() {
        let sha = "a".repeat(40);
        assert_eq!(copied_from_path(&format!("git@github.com:o/r.git@{sha}:src/a b.py")), Some("src/a b.py"));
        assert_eq!(copied_from_path("nothing"), None);
    }
}
