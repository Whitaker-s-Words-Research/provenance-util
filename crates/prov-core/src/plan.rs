//! The plan of `prov cp`: each file, and for each file the line ranges with
//! their authors.
//!
//! ```toml
//! [source]
//! repo = "/home/me/other-repo"
//! commit = "4f2a9c1e..."
//!
//! [[file]]
//! path = "src/parse.py"
//! dest = "src/parse.py"
//! ranges = [
//!   { lines = "1-4",   author = "human" },
//!   { lines = "5-9",   author = "ai", agent = "claude", model = "m1", session = "s1" },
//!   { lines = "10-12", author = "?" },
//! ]
//! ```

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// The value of `--agent` and `--model` when none is given.
pub const UNKNOWN: &str = "unknown";

/// The author of a range.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "author", rename_all = "lowercase")]
pub enum Who {
    /// The person who copies.
    Human,
    Ai {
        /// git-ai's name for the tool.
        agent: String,
        model: String,
        /// The conversation ID.
        session: String,
    },
    /// `author = "?"`: no author yet.
    #[serde(rename = "?")]
    Unresolved,
}

impl fmt::Display for Who {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Who::Human => write!(f, "human"),
            Who::Ai { agent, model, session } => write!(f, "ai {agent}/{model} ({session})"),
            Who::Unresolved => write!(f, "?"),
        }
    }
}

/// Lines `start` to `end` of a file, both included. The first line is 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: u32,
    pub end: u32,
    #[serde(flatten)]
    pub who: Who,
}

impl Range {
    pub fn lines(&self) -> u32 {
        self.end.saturating_sub(self.start) + 1
    }

    fn label(&self) -> String {
        if self.start == self.end {
            self.start.to_string()
        } else {
            format!("{}-{}", self.start, self.end)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// The path of the local clone.
    pub repo: String,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanFile {
    /// Relative to the root of the source.
    pub path: String,
    /// Relative to the root of this worktree.
    pub dest: String,
    /// Copied unchanged, with no line authorship.
    pub binary: bool,
    /// A symbolic link. Copied unchanged, with no line authorship.
    pub symlink: bool,
    pub ranges: Vec<Range>,
}

impl PlanFile {
    /// True if the file has line authorship.
    pub fn is_text(&self) -> bool {
        !self.binary && !self.symlink
    }

    /// The ranges of a file cover every line once.
    pub fn check_cover(&self, line_count: u32) -> Result<()> {
        if !self.is_text() {
            if !self.ranges.is_empty() {
                bail!("{}: a binary file or a symbolic link has no ranges", self.dest);
            }
            return Ok(());
        }
        let mut ranges: Vec<&Range> = self.ranges.iter().collect();
        ranges.sort_by_key(|r| r.start);
        let mut next = 1u32;
        for r in ranges {
            if r.start < 1 || r.end < r.start {
                bail!("{}: `{}` is not a range of lines", self.dest, r.label());
            }
            if r.start < next {
                bail!("{}: line {} is in two ranges", self.dest, r.start);
            }
            if r.start > next {
                bail!("{}: no range has the lines {}-{}", self.dest, next, r.start - 1);
            }
            next = r.end + 1;
        }
        if next - 1 < line_count {
            bail!("{}: no range has the lines {}-{}", self.dest, next, line_count);
        }
        if next - 1 > line_count {
            bail!("{}: the ranges end at line {}, but the file has {} lines", self.dest, next - 1, line_count);
        }
        Ok(())
    }

    /// The author of each line, in line order. Call after [`Self::check_cover`].
    pub fn line_authors(&self) -> Vec<&Who> {
        let mut ranges: Vec<&Range> = self.ranges.iter().collect();
        ranges.sort_by_key(|r| r.start);
        ranges.into_iter().flat_map(|r| std::iter::repeat_n(&r.who, r.lines() as usize)).collect()
    }
}

/// Joins the authors of consecutive lines into ranges.
pub fn ranges_of(authors: &[Who]) -> Vec<Range> {
    let mut out: Vec<Range> = Vec::new();
    for (i, who) in authors.iter().enumerate() {
        let line = u32::try_from(i).map_or(u32::MAX, |n| n.saturating_add(1));
        match out.last_mut() {
            Some(last) if last.who == *who => last.end = line,
            _ => out.push(Range { start: line, end: line, who: who.clone() }),
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub source: Source,
    pub files: Vec<PlanFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlan {
    source: RawSource,
    #[serde(default)]
    file: Vec<RawFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSource {
    repo: String,
    commit: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    path: String,
    dest: String,
    #[serde(default)]
    binary: bool,
    #[serde(default)]
    symlink: bool,
    #[serde(default)]
    ranges: Vec<RawRange>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRange {
    lines: String,
    author: String,
    agent: Option<String>,
    model: Option<String>,
    session: Option<String>,
}

/// `"5-9"` or `"7"`.
fn parse_lines(text: &str) -> Option<(u32, u32)> {
    let (start, end) = if let Some((a, b)) = text.split_once('-') { (a.trim().parse().ok()?, b.trim().parse().ok()?) } else {
        let n = text.trim().parse().ok()?;
        (n, n)
    };
    (start >= 1 && end >= start).then_some((start, end))
}

fn quote(text: &str) -> String {
    toml::Value::String(text.to_string()).to_string()
}

impl Plan {
    /// Parses a plan file. An `ai` range with no `agent` or `model` gets
    /// `unknown`; with no `session`, it gets `default_session`.
    pub fn parse(text: &str, default_session: &str) -> Result<Plan> {
        let raw: RawPlan = toml::from_str(text).context("the plan does not parse")?;
        let mut files = Vec::new();
        for f in raw.file {
            let mut ranges = Vec::new();
            for r in f.ranges {
                let (start, end) = parse_lines(&r.lines)
                    .with_context(|| format!("{}: `lines = {:?}` is not a range such as \"5-9\"", f.dest, r.lines))?;
                let who = match r.author.as_str() {
                    "human" | "?" => {
                        if r.agent.is_some() || r.model.is_some() || r.session.is_some() {
                            bail!("{}: lines {}: `agent`, `model` and `session` go with author = \"ai\"", f.dest, r.lines);
                        }
                        if r.author == "human" {
                            Who::Human
                        } else {
                            Who::Unresolved
                        }
                    }
                    "ai" => Who::Ai {
                        agent: r.agent.unwrap_or_else(|| UNKNOWN.into()),
                        model: r.model.unwrap_or_else(|| UNKNOWN.into()),
                        session: r.session.unwrap_or_else(|| default_session.into()),
                    },
                    other => bail!(
                        "{}: lines {}: author = {other:?}. Use \"human\", \"ai\" or \"?\".",
                        f.dest,
                        r.lines
                    ),
                };
                ranges.push(Range { start, end, who });
            }
            ranges.sort_by_key(|r| r.start);
            files.push(PlanFile { path: f.path, dest: f.dest, binary: f.binary, symlink: f.symlink, ranges });
        }
        Ok(Plan { source: Source { repo: raw.source.repo, commit: raw.source.commit }, files })
    }

    /// The text of the plan file.
    pub fn to_toml(&self) -> String {
        let mut out = format!("[source]\nrepo = {}\ncommit = {}\n", quote(&self.source.repo), quote(&self.source.commit));
        for f in &self.files {
            let _ = write!(out, "\n[[file]]\npath = {}\ndest = {}\n", quote(&f.path), quote(&f.dest));
            if f.binary {
                out.push_str("binary = true\n");
            }
            if f.symlink {
                out.push_str("symlink = true\n");
            }
            if !f.is_text() {
                continue;
            }
            if f.ranges.is_empty() {
                out.push_str("ranges = []\n");
                continue;
            }
            let width = f.ranges.iter().map(|r| r.label().len()).max().unwrap_or(0) + 3;
            out.push_str("ranges = [\n");
            for r in &f.ranges {
                let lines = format!("{},", quote(&r.label()));
                let author = match &r.who {
                    Who::Human => "author = \"human\"".to_string(),
                    Who::Unresolved => "author = \"?\"".to_string(),
                    Who::Ai { agent, model, session } => format!(
                        "author = \"ai\", agent = {}, model = {}, session = {}",
                        quote(agent),
                        quote(model),
                        quote(session)
                    ),
                };
                let _ = writeln!(out, "  {{ lines = {lines:width$} {author} }},");
            }
            out.push_str("]\n");
        }
        out
    }

    /// Each range with no author: the destination and the range.
    pub fn unresolved(&self) -> Vec<(&str, &Range)> {
        self.files
            .iter()
            .flat_map(|f| f.ranges.iter().filter(|r| r.who == Who::Unresolved).map(move |r| (f.dest.as_str(), r)))
            .collect()
    }

    /// git-ai keeps one model for each session. A plan that gives one session
    /// two models fails.
    pub fn check_models(&self) -> Result<()> {
        let mut models: BTreeMap<(&str, &str), &str> = BTreeMap::new();
        for f in &self.files {
            for r in &f.ranges {
                let Who::Ai { agent, model, session } = &r.who else { continue };
                match models.insert((agent, session), model) {
                    Some(other) if other != model => bail!(
                        "the session `{session}` of `{agent}` has two models in the plan: `{other}` and `{model}`. \
                         git-ai keeps one model for each session."
                    ),
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Two files cannot have the same destination.
    pub fn check_dests(&self) -> Result<()> {
        let mut seen: Vec<&str> = Vec::new();
        for f in &self.files {
            if seen.contains(&f.dest.as_str()) {
                bail!("two files of the plan have the destination {}", f.dest);
            }
            seen.push(&f.dest);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ai(agent: &str, model: &str, session: &str) -> Who {
        Who::Ai { agent: agent.into(), model: model.into(), session: session.into() }
    }

    const SAMPLE: &str = r#"
[source]
repo = "/home/me/other-repo"
commit = "4f2a9c1e"

[[file]]
path = "src/parse.py"
dest = "lib/parse.py"
ranges = [
  { lines = "1-4",   author = "human" },
  { lines = "5-9",   author = "ai", agent = "claude", model = "m1", session = "s1" },
  { lines = "10-12", author = "?" },
  { lines = "13",    author = "ai" },
]

[[file]]
path = "img/logo.png"
dest = "img/logo.png"
binary = true
"#;

    #[test]
    fn parses_the_format_of_the_spec() {
        let plan = Plan::parse(SAMPLE, "plan-1").unwrap();
        assert_eq!(plan.source.commit, "4f2a9c1e");
        let f = &plan.files[0];
        assert_eq!((f.path.as_str(), f.dest.as_str()), ("src/parse.py", "lib/parse.py"));
        assert_eq!(f.ranges[0], Range { start: 1, end: 4, who: Who::Human });
        assert_eq!(f.ranges[1].who, ai("claude", "m1", "s1"));
        assert_eq!(f.ranges[2].who, Who::Unresolved);
        // The defaults of an `ai` range.
        assert_eq!(f.ranges[3], Range { start: 13, end: 13, who: ai("unknown", "unknown", "plan-1") });
        f.check_cover(13).unwrap();
        assert!(plan.files[1].binary && plan.files[1].ranges.is_empty());
        plan.files[1].check_cover(0).unwrap();
        assert_eq!(plan.unresolved().len(), 1);
        assert_eq!(plan.unresolved()[0].0, "lib/parse.py");
    }

    #[test]
    fn the_text_parses_to_the_same_plan() {
        let plan = Plan::parse(SAMPLE, "plan-1").unwrap();
        let text = plan.to_toml();
        assert!(text.contains("  { lines = \"1-4\",   author = \"human\" },\n"), "{text}");
        assert!(text.contains("  { lines = \"10-12\", author = \"?\" },\n"), "{text}");
        assert_eq!(Plan::parse(&text, "other").unwrap(), plan);
    }

    #[test]
    fn ranges_cover_every_line_once() {
        let file = |ranges: &[(u32, u32)]| PlanFile {
            path: "a".into(),
            dest: "a".into(),
            binary: false,
            symlink: false,
            ranges: ranges.iter().map(|&(start, end)| Range { start, end, who: Who::Human }).collect(),
        };
        file(&[(1, 3), (4, 4)]).check_cover(4).unwrap();
        file(&[(4, 4), (1, 3)]).check_cover(4).unwrap();
        file(&[]).check_cover(0).unwrap();
        let err = |f: PlanFile, n| f.check_cover(n).unwrap_err().to_string();
        assert!(err(file(&[(1, 3), (3, 4)]), 4).contains("line 3 is in two ranges"));
        assert!(err(file(&[(1, 2), (4, 4)]), 4).contains("no range has the lines 3-3"));
        assert!(err(file(&[(1, 2)]), 4).contains("no range has the lines 3-4"));
        assert!(err(file(&[(1, 5)]), 4).contains("the file has 4 lines"));
        assert!(err(file(&[(2, 4)]), 4).contains("no range has the lines 1-1"));
    }

    #[test]
    fn one_session_has_one_model() {
        let text = SAMPLE.replace("{ lines = \"13\",    author = \"ai\" }", "{ lines = \"13\", author = \"ai\", agent = \"claude\", model = \"m2\", session = \"s1\" }");
        let plan = Plan::parse(&text, "p").unwrap();
        assert!(plan.check_models().unwrap_err().to_string().contains("two models"));
        Plan::parse(SAMPLE, "p").unwrap().check_models().unwrap();
    }

    #[test]
    fn bad_plans() {
        let bad = |from: &str, to: &str| Plan::parse(&SAMPLE.replace(from, to), "p").unwrap_err();
        assert!(format!("{:#}", bad("author = \"human\"", "author = \"me\"")).contains("author = \"me\""));
        assert!(format!("{:#}", bad("\"1-4\"", "\"4-1\"")).contains("not a range"));
        assert!(format!("{:#}", bad("author = \"human\"", "author = \"human\", model = \"m\"")).contains("go with author"));
        assert!(Plan::parse(&SAMPLE.replace("binary = true", "bogus = 1"), "p").is_err());
    }

    #[test]
    fn line_authors_and_back() {
        let authors = vec![Who::Human, Who::Human, ai("a", "m", "s"), Who::Human];
        let ranges = ranges_of(&authors);
        assert_eq!(ranges.len(), 3);
        assert_eq!((ranges[1].start, ranges[1].end), (3, 3));
        let file = PlanFile { path: "a".into(), dest: "a".into(), binary: false, symlink: false, ranges };
        assert_eq!(file.line_authors().into_iter().cloned().collect::<Vec<_>>(), authors);
    }
}
