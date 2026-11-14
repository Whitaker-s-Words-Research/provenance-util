//! The rules of the merge gate. `prepare-commit-msg` applies them to a merge
//! in progress, and `prov check` applies them without a merge.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};
use prov_messages as msg;
use prov_core::trailer::{self, Approval, SessionRef, Trailer};
use prov_core::{Config, SessionRecord, State};
use prov_git::{CommitInfo, PathChange, Repo};
use prov_gitai::{Author, GitAi};
use serde::Serialize;

use crate::context::{full_version, say, Ctx};

/// A human commit: its author email is not an agent email. A commit with a
/// `Prov-Session` trailer is a human commit only if it also has a
/// `Prov-Version` trailer, which marks an integration.
pub fn is_human_commit(commit: &CommitInfo, agent_emails: &[String]) -> bool {
    let agent_email = agent_emails.contains(&commit.author_email.to_ascii_lowercase());
    let session = trailer::has(&commit.trailers, trailer::PROV_SESSION);
    let version = trailer::has(&commit.trailers, trailer::PROV_VERSION);
    !agent_email && (!session || version)
}

/// True if a merge of `tip` into `base` is an integration: one commit on the
/// first-parent line of `base..tip` is not a human commit. The decision comes
/// from the incoming commits, not from a branch name.
pub fn is_integration(repo: &Repo, base: &str, tip: &str, agent_emails: &[String]) -> Result<bool> {
    let line = repo.commits(["--first-parent", &format!("{base}..{tip}")])?;
    Ok(line.iter().any(|c| !is_human_commit(c, agent_emails)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// No range gives the lines a session or a human.
    Untracked,
    /// The agent commit has no git-ai note, so no line of it has an author.
    NoNote,
    /// A range gives the lines a human. In an agent commit, this label comes
    /// from git-ai's fallback, not from a person.
    Human,
    /// A change to a protected path.
    Protected,
    /// The note gives the lines to an AI session that no `Prov-Session`
    /// trailer names and that has no record. git-ai invents such a session
    /// for lines with no checkpoint in a commit with an agent identity, for
    /// example after `git commit --no-verify`. No transcript exists for it.
    UnknownSession,
    /// A merge commit in the incoming range changed the path relative to
    /// every parent: the author of the merge wrote the result, for example to
    /// resolve a conflict. git-ai gives no usable line record for a merge.
    MergeResolution,
    /// git shows added lines, a binary change or a new mode for the file,
    /// and git-ai lists no range of it. git-ai ignores some paths, for
    /// example `**/vendor/**` and lock files: it drops their checkpoints and
    /// lists none of their lines. It also lists no range for a binary file
    /// or a change of mode.
    NoRanges,
    /// The agent commit deletes the file.
    DeletedFile,
}

/// Something that the human must approve before the integration.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub path: String,
    pub reason: Reason,
    /// The agent commit of a doubtful range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    /// The session of an `UnknownSession` range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

impl Item {
    pub fn lines(&self) -> u32 {
        match (self.start_line, self.end_line) {
            (Some(start), Some(end)) => end.saturating_sub(start) + 1,
            _ => 0,
        }
    }

    pub fn describe(&self) -> String {
        let commit = self.commit.as_deref().map_or("?", |c| &c[..c.len().min(10)]);
        let path = &self.path;
        let (start, end) = (self.start_line.unwrap_or(0), self.end_line.unwrap_or(0));
        match self.reason {
            Reason::Protected => msg::gate::item_protected(path),
            Reason::MergeResolution => msg::gate::item_merge_resolution(path, commit),
            Reason::NoRanges => msg::gate::item_no_ranges(path, commit),
            Reason::DeletedFile => msg::gate::item_deleted_file(path, commit),
            Reason::UnknownSession if self.session.as_deref().is_some_and(|s| s.starts_with(prov_gitai::UNATTRIBUTED_TOOL)) => {
                msg::gate::item_tainted(path, start, end, commit)
            }
            Reason::UnknownSession => {
                msg::gate::item_unknown_session(path, start, end, commit, self.session.as_deref().unwrap_or("?"))
            }
            Reason::NoNote => msg::gate::item_no_note(path, start, end, commit),
            Reason::Human => msg::gate::item_human(path, start, end, commit),
            Reason::Untracked => msg::gate::item_untracked(path, start, end, commit),
        }
    }
}

/// What a merge of `tip` into `base` brings in.
#[derive(Debug, Default)]
pub struct Analysis {
    /// The incoming range `base..tip`, oldest first. Every one is an agent commit.
    pub commits: Vec<CommitInfo>,
    /// The sessions named in a `Prov-Session` trailer or in the note of an
    /// agent commit, in order of first appearance.
    pub sessions: Vec<SessionRef>,
    pub items: Vec<Item>,
    /// The sessions that a `Prov-Session` trailer names.
    named: Vec<SessionRef>,
    /// Each range that a note gives to an AI session.
    ai_ranges: Vec<(SessionRef, Item)>,
}

impl Analysis {
    /// Removes each session that no `Prov-Session` trailer names and that
    /// `known` does not know. Its lines become doubtful ranges, which a human
    /// approves by path. The merge gate knows a session by its record;
    /// `prov check <base>..<tip>` knows it by its trailer group.
    pub fn drop_unknown_sessions(&mut self, known: impl Fn(&SessionRef) -> bool) {
        let named = &self.named;
        let (keep, unknown): (Vec<SessionRef>, Vec<SessionRef>) =
            std::mem::take(&mut self.sessions).into_iter().partition(|s| named.contains(s) || known(s));
        self.sessions = keep;
        for (session, item) in &self.ai_ranges {
            if unknown.contains(session) {
                self.items.push(item.clone());
            }
        }
    }

    pub fn doubtful_lines(&self) -> u32 {
        self.items.iter().map(Item::lines).sum()
    }
}

/// True if the commit has a git-ai note. This reads git; the content of the
/// note comes only from git-ai.
fn has_note(repo: &Repo, commit: &str) -> Result<bool> {
    repo.git_ok(["notes", "--ref=ai", "list", commit])
}

/// True if the commit has a note, after a wait for a recent commit.
///
/// The caller ran `git-ai await` before. If a commit of the last two
/// minutes still has no note, this waits for it, up to the time limit of
/// `git-ai await`. The limit holds for all waits of one analysis together:
/// `waiting_since` is the start of the first wait. An older commit with no
/// note will not get one: there is no wait. Reason: on a slow file system
/// under load, a test once saw a commit with no note after the await; the
/// cause is not known.
fn wait_for_note(repo: &Repo, commit: &str, waiting_since: &mut Option<std::time::Instant>) -> Result<bool> {
    if has_note(repo, commit)? {
        return Ok(true);
    }
    let committed: u64 = repo.git(["log", "-1", "--format=%ct", commit])?.trim().parse().unwrap_or(0);
    if prov_core::now().saturating_sub(committed) > 120 {
        return Ok(false);
    }
    let begin = *waiting_since.get_or_insert_with(std::time::Instant::now);
    let limit = std::time::Duration::from_secs(u64::from(prov_gitai::AWAIT_TIMEOUT_SECS));
    while begin.elapsed() < limit {
        std::thread::sleep(std::time::Duration::from_millis(250));
        if has_note(repo, commit)? {
            say(msg::gate::late_note(commit, format!("{:?}", begin.elapsed())));
            return Ok(true);
        }
    }
    Ok(false)
}

/// Reads each agent commit of `base..tip` with `git-ai diff <commit> --json`.
/// `config` supplies the protected paths.
pub fn analyze(repo: &Repo, gitai: &GitAi, config: &Config, base: &str, tip: &str) -> Result<Analysis> {
    let commits = repo.commits(["--reverse", "--topo-order", &format!("{base}..{tip}")])?;
    let mut sessions: Vec<SessionRef> = Vec::new();
    let add = |sessions: &mut Vec<SessionRef>, s: SessionRef| {
        if !sessions.contains(&s) {
            sessions.push(s);
        }
    };
    let mut items = Vec::new();
    let mut named = Vec::new();
    let mut ai_ranges = Vec::new();
    let mut waiting_since = None;
    for commit in &commits {
        for s in trailer::sessions(&commit.trailers) {
            add(&mut named, s.clone());
            add(&mut sessions, s);
        }
        // git-ai shows a merge commit as the difference from its first parent,
        // which repeats the lines of commits that are read on their own. What
        // the merge itself wrote needs an approval by path.
        if commit.parents.len() > 1 {
            for path in repo.merge_resolved_paths(&commit.sha)? {
                items.push(Item {
                    path,
                    reason: Reason::MergeResolution,
                    commit: Some(commit.sha.clone()),
                    start_line: None,
                    end_line: None,
                    session: None,
                });
            }
            continue;
        }
        let has_note = wait_for_note(repo, &commit.sha, &mut waiting_since)?;
        let authorship = gitai.diff(&repo.toplevel, &commit.sha)?;
        for s in &authorship.sessions {
            add(&mut sessions, SessionRef::new(&s.tool, &s.id));
        }
        // A deleted file, and a changed file that git-ai does not list at all.
        // A file that only loses lines needs no approval: a deleted line has
        // no author to doubt.
        for (path, change) in repo.path_changes(&commit.sha)? {
            let reason = match change {
                PathChange::Deleted => Reason::DeletedFile,
                PathChange::Content if !authorship.added.iter().any(|r| r.path == path) => Reason::NoRanges,
                PathChange::Content | PathChange::LinesDeleted => continue,
            };
            items.push(Item { path, reason, commit: Some(commit.sha.clone()), start_line: None, end_line: None, session: None });
        }
        for range in authorship.added {
            let item = |reason, session| Item {
                path: range.path.clone(),
                reason,
                commit: Some(commit.sha.clone()),
                start_line: Some(range.start),
                end_line: Some(range.end),
                session,
            };
            match &range.author {
                Author::Ai(s) => {
                    let session = SessionRef::new(&s.tool, &s.id);
                    let text = session.to_string();
                    ai_ranges.push((session, item(Reason::UnknownSession, Some(text))));
                }
                Author::Human => items.push(item(Reason::Human, None)),
                Author::Untracked if !has_note => items.push(item(Reason::NoNote, None)),
                Author::Untracked => items.push(item(Reason::Untracked, None)),
            }
        }
    }
    let protected = config.protected_set()?;
    for path in repo.changed_paths(base, tip)? {
        if protected.matches(&path) {
            items.push(Item { path, reason: Reason::Protected, commit: None, start_line: None, end_line: None, session: None });
        }
    }
    Ok(Analysis { commits, sessions, items, named, ai_ranges })
}

/// The items whose path has no `Prov-Approved` trailer.
pub fn unapproved<'a>(items: &'a [Item], approvals: &[Approval]) -> Vec<&'a Item> {
    items.iter().filter(|item| !approvals.iter().any(|a| a.path == item.path)).collect()
}

/// The record of the session in this clone. An ID that cannot be a record
/// name has no record. A record that cannot be read or parsed is an error,
/// not a missing record.
pub fn record_of(ctx: &Ctx, session: &SessionRef) -> Result<Option<SessionRecord>> {
    if !prov_core::state::is_record_name(&session.id) {
        return Ok(None);
    }
    Ok(ctx.store.get(&session.id)?.filter(|r| r.agent == session.agent))
}

/// Removes each session with no record in this clone and no trailer (see
/// [`Analysis::drop_unknown_sessions`]).
pub fn drop_sessions_without_record(ctx: &Ctx, analysis: &mut Analysis) -> Result<()> {
    let mut known = Vec::new();
    for s in &analysis.sessions {
        if record_of(ctx, s)?.is_some() {
            known.push(s.clone());
        }
    }
    analysis.drop_unknown_sessions(|s| known.contains(s));
    Ok(())
}

/// Checks the sessions of an integration against live state. Returns the
/// records of the sessions, in start order, and the problems.
///
/// - Each session must have a record and be Archived.
/// - No session may be Open in a worktree whose branch tip is in the incoming range.
pub fn check_sessions(ctx: &Ctx, analysis: &Analysis) -> Result<(Vec<SessionRecord>, Vec<String>)> {
    let mut records = Vec::new();
    let mut problems = Vec::new();
    for session in &analysis.sessions {
        let record = record_of(ctx, session)?.map(|r| r.at(prov_core::now()));
        match record {
            None => problems.push(msg::gate::session_no_record(session)),
            Some(r) if r.state == State::Archived && r.archive_sha256.as_deref().is_some_and(trailer::is_sha256) => {
                records.push(r);
            }
            Some(r) => problems.push(match r.state {
                State::Open => msg::gate::session_open(session, &r.id),
                State::Closing => msg::gate::session_closing(session),
                State::Failed => msg::gate::session_failed(session, r.error.as_deref().unwrap_or(msg::gate::NO_ERROR_TEXT), &r.id),
                State::Archived => msg::gate::session_no_hash(session),
            }),
        }
    }
    let incoming: BTreeSet<&str> = analysis.commits.iter().map(|c| c.sha.as_str()).collect();
    for r in ctx.store.sessions()? {
        if r.state != State::Open || analysis.sessions.iter().any(|s| s.id == r.id && s.agent == r.agent) {
            continue;
        }
        if let Some(tip) = ctx.repo.rev_parse(&format!("refs/heads/{}", r.branch))? {
            if incoming.contains(tip.as_str()) {
                problems.push(msg::gate::session_open_in_range(
                    SessionRef::new(&r.agent, &r.id),
                    r.worktree.display(),
                    &r.branch,
                    &r.id,
                ));
            }
        }
    }
    records.sort_by(|a, b| (a.started_at, &a.id).cmp(&(b.started_at, &b.id)));
    Ok((records, problems))
}

/// The integration trailers: one group for each session, in start order, then `Prov-Version`.
pub fn integration_trailers(records: &[SessionRecord]) -> Result<Vec<Trailer>> {
    let mut out = Vec::new();
    for r in records {
        out.push(Trailer::new(trailer::PROV_SESSION, SessionRef::new(&r.agent, &r.id).to_string())?);
        let hash = r.archive_sha256.clone().context(msg::gate::ARCHIVED_NO_HASH)?;
        out.push(Trailer::new(trailer::PROV_TRANSCRIPT_SHA256, hash)?);
        // The note can lack the model: git-ai takes it only from a checkpoint
        // that records a change, and the agent's own hooks give none.
        if !r.model.is_empty() && r.model != "unknown" {
            out.push(Trailer::new(trailer::PROV_MODEL, r.model.clone())?);
        }
    }
    out.push(Trailer::new(trailer::PROV_VERSION, full_version())?);
    Ok(out)
}

/// Removes the integration trailers from a message file, so that they can go
/// in again as one complete block.
fn strip_integration_trailers(message_file: &Path) -> Result<()> {
    let text = std::fs::read_to_string(message_file).with_context(|| msg::fs::cannot_read(message_file.display()))?;
    let keys = [trailer::PROV_SESSION, trailer::PROV_TRANSCRIPT_SHA256, trailer::PROV_MODEL, trailer::PROV_VERSION];
    let kept: Vec<&str> =
        text.lines().filter(|line| !keys.iter().any(|k| line.strip_prefix(k).is_some_and(|rest| rest.starts_with(':')))).collect();
    let mut out = kept.join("\n");
    out.push('\n');
    std::fs::write(message_file, out).with_context(|| msg::fs::cannot_write(message_file.display()))
}

/// The gate, for the merge in progress. Returns false to refuse the commit.
///
/// `heads` are the commits of `MERGE_HEAD`. If the merge is not an
/// integration, the gate does not run and the commit is allowed.
pub fn run(ctx: &Ctx, message_file: &Path, heads: &[String]) -> Result<bool> {
    let repo = &ctx.repo;
    // The config comes from HEAD: the file in the worktree is already the
    // merged file, with the agent's changes.
    let head_config = repo.config_at("HEAD")?;
    let emails = prov_adapters::agent_emails(head_config.as_ref().unwrap_or(&Config::default()));
    let mut any = false;
    for head in heads {
        any |= is_integration(repo, "HEAD", head, &emails)?;
    }
    if !any {
        return Ok(true);
    }
    if heads.len() > 1 {
        say(msg::gate::OCTOPUS);
        return Ok(false);
    }
    let tip = &heads[0];

    // 1. The config and the version.
    let Some(config) = head_config else {
        say(msg::gate::NO_CONFIG);
        return Ok(false);
    };
    if let Err(e) = config.check_version(prov_core::VERSION) {
        say(msg::gate::version_fails(e));
        return Ok(false);
    }

    // 2. git-ai writes a note some seconds after a commit. Under load,
    // `git-ai await` once gave up early with all notes written. The gate does
    // not depend on it: `analyze` waits for the note of each recent commit,
    // and a commit with no note gives items that need an approval.
    if let Err(e) = ctx.gitai.await_notes(&repo.toplevel) {
        say(msg::gate::await_failed(format!("{e:#}")));
    }

    // 3. The sessions.
    let mut analysis = analyze(repo, &ctx.gitai, &config, "HEAD", tip)?;
    // R-DISC-1: hold the lock from the first read of a record until the
    // trailers are written. `prov discard` deletes records and archives under
    // the lock, and refuses while a merge of the branch is in progress. git
    // writes MERGE_HEAD before this hook, so a discard either sees the merge
    // or has deleted the records before this read. Taken after `analyze`,
    // which is slow; nothing below takes the lock again.
    let _lock = ctx.store.lock()?;
    drop_sessions_without_record(ctx, &mut analysis)?;
    let (records, problems) = check_sessions(ctx, &analysis)?;

    // 4. Doubtful ranges and protected paths.
    let message = repo.message_trailers(message_file)?;
    let approvals = trailer::approvals(&message);
    let open = unapproved(&analysis.items, &approvals);

    if !problems.is_empty() || !open.is_empty() {
        say(msg::gate::REFUSED);
        for p in &problems {
            say(msg::gate::problem(p));
        }
        if open.is_empty() {
            say(msg::gate::FIX_SESSIONS);
        } else {
            say(msg::gate::ITEMS_HEADER);
            for item in &open {
                say(msg::gate::item(item.describe()));
            }
            let paths: BTreeSet<&str> = open.iter().map(|i| i.path.as_str()).collect();
            say(msg::gate::NEXT_STEPS);
            say(msg::gate::step_approve(paths.iter().map(|p| shell_quote(p)).collect::<Vec<_>>().join(" ")));
            say(msg::gate::STEP_COMMIT);
        }
        say(msg::gate::TO_STOP);
        return Ok(false);
    }

    // The message is complete if it has one group for each session, with
    // the hash of the record, and `Prov-Version`.
    let groups = trailer::session_groups(&message);
    let complete = trailer::has(&message, trailer::PROV_VERSION)
        && groups.len() == records.len()
        && records.iter().all(|r| {
            groups.iter().any(|(s, hash)| s.id == r.id && s.agent == r.agent && hash.as_deref() == r.archive_sha256.as_deref())
        });
    // The gate can run twice on one message, for example when the editor
    // stops the first commit. The trailers go in once, as one block. The
    // human can still change the message in the editor: `post-commit`
    // reads the commit that was made, and `prov check` reports a merge with
    // a missing group (an accepted limit).
    if !complete {
        if !groups.is_empty() || trailer::has(&message, trailer::PROV_VERSION) {
            strip_integration_trailers(message_file)?;
        }
        repo.add_trailers(message_file, &integration_trailers(&records)?)?;
    }
    Ok(true)
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./+=:@".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(email: &str, trailers: &str) -> CommitInfo {
        CommitInfo {
            sha: "a".repeat(40),
            author_email: email.into(),
            parents: vec![],
            subject: "s".into(),
            trailers: trailer::parse_block(trailers),
        }
    }

    #[test]
    fn human_commits() {
        let agents = vec!["noreply@anthropic.com".to_string()];
        assert!(is_human_commit(&commit("me@example.invalid", ""), &agents));
        assert!(!is_human_commit(&commit("NoReply@Anthropic.com", ""), &agents));
        // A Prov-Session trailer marks an agent commit, whatever the email ...
        assert!(!is_human_commit(&commit("me@example.invalid", "Prov-Session: claude:s1\n"), &agents));
        // ... except on an integration, which also has Prov-Version.
        let integration = "Prov-Session: claude:s1\nProv-Transcript-SHA256: ab\nProv-Version: 0.1.0+x\n";
        assert!(is_human_commit(&commit("me@example.invalid", integration), &agents));
        assert!(!is_human_commit(&commit("noreply@anthropic.com", integration), &agents));
    }

    #[test]
    fn approval_is_by_path() {
        let item = |path: &str, reason| Item { path: path.into(), reason, commit: None, start_line: Some(1), end_line: Some(3), session: None };
        let items = vec![item("a b.txt", Reason::Human), item("c.txt", Reason::Untracked), item(".claude/x", Reason::Protected)];
        let approvals = vec![Approval::parse("a b.txt ack").unwrap(), Approval::parse(".claude/x ack").unwrap()];
        let open = unapproved(&items, &approvals);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].path, "c.txt");
        assert_eq!(items[0].lines(), 3);
        assert_eq!(shell_quote("a b.txt"), "'a b.txt'");
    }
}
