//! Path patterns with gitignore rules: `*` does not match `/`, `**` matches
//! across directories.
//!
//! The matcher is written here, with no regular expressions. Reason: a hook
//! builds the patterns at each start, and a hook has a time limit.
//!
//! Supported, as in gitignore:
//! - `*`, `?`, `[a-z]`, `[!a-z]` and `\` inside one path component
//! - `**` as a whole component: any number of directories
//! - a pattern with a `/` at the start or in the middle matches from the root;
//!   a pattern with no such `/` matches at every level
//! - a pattern that ends with `/` matches only a directory
//! - a pattern that matches a directory matches everything below it
//!
//! Not supported: negation with `!`. Such a pattern is an error.

use anyhow::{bail, Result};

/// The fixed pattern for agent branches.
pub const AGENT_BRANCH_PATTERN: &str = "agent/**";

#[derive(Debug, Clone)]
struct Pattern {
    segments: Vec<String>,
    dir_only: bool,
}

/// A set of gitignore-style patterns.
#[derive(Debug, Clone, Default)]
pub struct PathSet {
    patterns: Vec<Pattern>,
}

impl PathSet {
    pub fn new<S: AsRef<str>>(patterns: &[S]) -> Result<Self> {
        let mut out = Vec::new();
        for raw in patterns {
            let raw = raw.as_ref();
            let text = raw.trim();
            if text.is_empty() {
                bail!("invalid pattern `{raw}`: empty");
            }
            if text.starts_with('!') {
                bail!("invalid pattern `{raw}`: negation is not supported");
            }
            let dir_only = text.ends_with('/');
            let body = text.trim_end_matches('/');
            let anchored = body.contains('/');
            let body = body.trim_start_matches('/');
            if body.is_empty() {
                bail!("invalid pattern `{raw}`");
            }
            let mut segments: Vec<String> = body.split('/').map(String::from).collect();
            if segments.iter().any(String::is_empty) {
                bail!("invalid pattern `{raw}`: empty path component");
            }
            if !anchored && segments[0] != "**" {
                segments.insert(0, "**".to_string());
            }
            out.push(Pattern { segments, dir_only });
        }
        Ok(Self { patterns: out })
    }

    /// True if `path` (relative, `/`-separated, a file) or one of its parent
    /// directories matches a pattern.
    pub fn matches(&self, path: &str) -> bool {
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            return false;
        }
        self.patterns.iter().any(|pattern| {
            (1..=parts.len()).any(|end| {
                let is_dir = end < parts.len();
                (is_dir || !pattern.dir_only) && match_segments(&pattern.segments, &parts[..end])
            })
        })
    }
}

fn match_segments(pattern: &[String], path: &[&str]) -> bool {
    let Some((first, rest)) = pattern.split_first() else { return path.is_empty() };
    if first == "**" {
        if rest.is_empty() {
            // A trailing `**` matches everything inside, not the directory itself.
            return !path.is_empty();
        }
        return (0..=path.len()).any(|skip| match_segments(rest, &path[skip..]));
    }
    match path.split_first() {
        Some((name, path_rest)) => {
            glob(&first.chars().collect::<Vec<_>>(), &name.chars().collect::<Vec<_>>()) && match_segments(rest, path_rest)
        }
        None => false,
    }
}

/// Matches one path component.
fn glob(pattern: &[char], text: &[char]) -> bool {
    let Some((&p, pattern_rest)) = pattern.split_first() else { return text.is_empty() };
    match p {
        '*' => (0..=text.len()).any(|skip| glob(pattern_rest, &text[skip..])),
        '?' => !text.is_empty() && glob(pattern_rest, &text[1..]),
        '[' => match (class(pattern_rest), text.split_first()) {
            (Some((set, negated, after)), Some((&c, text_rest))) => {
                let inside = set.iter().any(|&(low, high)| low <= c && c <= high);
                inside != negated && glob(after, text_rest)
            }
            // An unclosed `[` is a literal character.
            (None, Some((&c, text_rest))) => c == '[' && glob(pattern_rest, text_rest),
            (_, None) => false,
        },
        '\\' if !pattern_rest.is_empty() => {
            !text.is_empty() && text[0] == pattern_rest[0] && glob(&pattern_rest[1..], &text[1..])
        }
        literal => !text.is_empty() && text[0] == literal && glob(pattern_rest, &text[1..]),
    }
}

/// Parses a character class after its `[`. Returns the ranges, whether the
/// class is negated, and the pattern after the `]`.
#[allow(clippy::type_complexity)]
fn class(pattern: &[char]) -> Option<(Vec<(char, char)>, bool, &[char])> {
    let mut i = 0;
    let negated = matches!(pattern.first(), Some('!' | '^'));
    if negated {
        i += 1;
    }
    let mut set = Vec::new();
    let start = i;
    while i < pattern.len() {
        let c = pattern[i];
        if c == ']' && i > start {
            return Some((set, negated, &pattern[i + 1..]));
        }
        if i + 2 < pattern.len() && pattern[i + 1] == '-' && pattern[i + 2] != ']' {
            set.push((c, pattern[i + 2]));
            i += 3;
        } else {
            set.push((c, c));
            i += 1;
        }
    }
    None
}

/// True if a branch name matches `agent/**`.
pub fn is_agent_branch(name: &str) -> bool {
    // The pattern is a constant, so the build cannot fail.
    PathSet::new(&[AGENT_BRANCH_PATTERN]).is_ok_and(|s| s.matches(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(patterns: &[&str]) -> PathSet {
        PathSet::new(patterns).unwrap()
    }

    #[test]
    fn agent_branches() {
        assert!(is_agent_branch("agent/claude/20261111-ab12"));
        assert!(is_agent_branch("agent/x"));
        assert!(!is_agent_branch("agent"));
        assert!(!is_agent_branch("main"));
        assert!(!is_agent_branch("feature/agent/x"));
        assert!(!is_agent_branch("agents/x"));
    }

    #[test]
    fn protected_defaults() {
        let set = set(&[".provenance.toml", ".claude/**", ".github/workflows/**", "CLAUDE.md", "AGENTS.md"]);
        assert!(set.matches(".provenance.toml"));
        assert!(set.matches(".claude/settings.json"));
        assert!(set.matches(".claude/hooks/a/b.sh"));
        assert!(set.matches(".github/workflows/ci.yml"));
        assert!(set.matches("CLAUDE.md"));
        assert!(set.matches("AGENTS.md"));
        assert!(set.matches("crates/prov/CLAUDE.md"));
        assert!(set.matches("docs/AGENTS.md"));
        assert!(!set.matches("CLAUDE.local.md"));
        assert!(!set.matches("docs/claude.md"));
        assert!(!set.matches(".github/CODEOWNERS"));
        assert!(!set.matches("src/main.rs"));
        assert!(!set.matches(".claude"));
    }

    #[test]
    fn star_does_not_cross_directories() {
        let s = set(&["/docs/*.md"]);
        assert!(s.matches("docs/a.md"));
        assert!(!s.matches("docs/sub/a.md"));
        assert!(!s.matches("x/docs/a.md"));
    }

    #[test]
    fn double_star_crosses_directories() {
        let s = set(&["src/**/gen.rs", "**/secrets/*.key"]);
        assert!(s.matches("src/gen.rs"));
        assert!(s.matches("src/a/b/gen.rs"));
        assert!(!s.matches("lib/gen.rs"));
        assert!(s.matches("secrets/a.key"));
        assert!(s.matches("x/y/secrets/a.key"));
        assert!(!s.matches("secrets/sub/a.key"));
    }

    #[test]
    fn a_pattern_with_no_slash_matches_at_every_level() {
        let s = set(&["*.lock", "build"]);
        assert!(s.matches("Cargo.lock"));
        assert!(s.matches("a/b/x.lock"));
        // A matched directory covers everything below it.
        assert!(s.matches("build/out/a.o"));
        assert!(s.matches("x/build/a.o"));
        assert!(!s.matches("builder/a.o"));
    }

    #[test]
    fn directory_only_patterns() {
        let s = set(&["ci/"]);
        assert!(s.matches("ci/run.sh"));
        assert!(s.matches("tools/ci/run.sh"));
        assert!(!s.matches("ci"));
        assert!(!s.matches("tools/ci"));
    }

    #[test]
    fn classes_escapes_and_question_marks() {
        let s = set(&["v[0-9].txt", "n[!a-c]", "lit\\*", "a?c", "odd[", "sp ace/*"]);
        assert!(s.matches("v7.txt") && !s.matches("vx.txt"));
        assert!(s.matches("nd") && !s.matches("nb"));
        assert!(s.matches("lit*") && !s.matches("literal"));
        assert!(s.matches("abc") && !s.matches("ac") && !s.matches("a/c"));
        assert!(s.matches("odd["));
        assert!(s.matches("sp ace/file"));
    }

    #[test]
    fn invalid_patterns() {
        assert!(PathSet::new(&["!keep"]).is_err());
        assert!(PathSet::new(&[""]).is_err());
        assert!(PathSet::new(&["a//b"]).is_err());
        assert!(PathSet::new(&["/"]).is_err());
        assert!(!PathSet::default().matches("anything"));
    }
}
