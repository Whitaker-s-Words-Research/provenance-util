//! `prov doctor`. The name of each check (`config`, `hooks`, ...) is a value
//! of the JSON output, so it stays in the code.

use std::fmt::Display;

pub fn version_problem(error: impl Display) -> String {
    format!("{error}. Install that version, or change required_version.")
}

pub fn no_config(file: impl Display, worktree: impl Display) -> String {
    format!("{file} does not exist in {worktree}. Run: prov init")
}

pub fn shim_not_executable(name: impl Display) -> String {
    format!("the shim {name} is not executable. Run: prov init")
}

pub fn not_shim(path: impl Display) -> String {
    format!("{path} is not the shim of prov. Run: prov init")
}

pub fn no_shim(name: impl Display, dir: impl Display) -> String {
    format!("the shim {name} does not exist in {dir}. Run: prov init")
}

pub fn git_option(key: impl Display, found: Option<impl Display>, value: impl Display) -> String {
    match found {
        Some(found) => format!("{key} is `{found}` in this repository, not `{value}`. Run: prov init"),
        None => format!("{key} is not set in this repository, not `{value}`. Run: prov init"),
    }
}

pub fn no_adapter(agent: impl Display) -> String {
    format!("no adapter for the agent `{agent}`")
}

pub fn no_identity(agent: impl Display) -> String {
    format!("no commit identity for the agent `{agent}`")
}

pub fn settings_problem(file: impl Display, problem: impl Display) -> String {
    format!("{file}: {problem}. Run: prov init")
}

pub fn not_tracked(file: impl Display) -> String {
    format!("{file} is not tracked. Commit it: an agent worktree gets it only from a commit.")
}

pub fn not_committed(file: impl Display) -> String {
    format!("{file} has changes that are not committed")
}

pub fn no_gitai(error: impl Display) -> String {
    format!("{error}. Install git-ai.")
}

pub fn gitai_unsupported(version: impl Display, baseline: impl Display) -> String {
    format!("git-ai is version {version}. The compatibility baseline is {baseline}.")
}

pub fn gitai_untested(version: impl Display, tested: impl Display) -> String {
    format!(
        "git-ai is version {version}, newer than {tested}, the newest version that the git-ai \
         contract tests of prov passed against. prov depends on how git-ai attributes lines; if that \
         changed, the gate can show wrong authors. Run the contract tests of prov against this version \
         (cargo test --test gitai_contract), or install git-ai {tested}."
    )
}

pub const GITAI_UNAVAILABLE: &str = "the git-ai service is not available. Start it: git-ai bg start";

pub fn cannot_read_gitai_config(error: impl Display) -> String {
    format!("cannot read the git-ai configuration: {error}")
}

pub fn gitai_debug_fails(output: impl Display) -> String {
    format!("`git-ai debug` fails:\n{output}")
}

pub const DEBUG_ONLY_DEEP: &str = "`git-ai debug` runs only with --deep";

pub fn integration_missing(agent: impl Display, problem: impl Display) -> String {
    format!("the git-ai integration of `{agent}` is not installed: {problem}")
}

pub fn session_merged(id: impl Display, state: impl Display, branch: impl Display) -> String {
    format!(
        "session {id} is {state}, and its branch {branch} is merged into this branch. If its work is integrated, \
         delete the record: prov session drop {id}"
    )
}

pub fn session_stale(id: impl Display) -> String {
    format!("session {id} is stale (no event for 24 hours, or its agent process is gone). Archive it: prov session archive {id}")
}

pub fn session_failed(id: impl Display, error: impl Display) -> String {
    format!("session {id} is failed: {error}. Run the archive step again: prov session archive {id}")
}

pub fn worktree_contended(worktree: impl Display, ids: impl Display) -> String {
    format!("the worktree {worktree} is contended: the sessions {ids} are open there. End all but one.")
}

pub fn worktree_record_removed(worktree: impl Display) -> String {
    format!("removed the record of the worktree {worktree}, which no longer exists")
}

pub fn copy_not_committed(count: impl Display, id: impl Display, worktree: impl Display) -> String {
    format!("{count} file(s) of the copy {id} in {worktree} are not committed yet")
}

/// A copied file whose check has no detail.
pub const NO_DETAIL: &str = "no detail";

/// `commit` is shortened to 10 characters.
pub fn copy_failed(dest: impl Display, commit: &str, detail: impl Display) -> String {
    format!(
        "the copy check failed for {dest} (commit {commit:.10}): {detail}.\nReplace its authorship: git rm the file \
         and commit, then copy it again with `prov cp`."
    )
}

pub fn copy_moved(dest: impl Display, commit: &str, detail: impl Display, record: impl Display) -> String {
    format!("{dest} (commit {commit:.10}): {detail}. The content and the counts are correct. To remove this note, delete {record}")
}

pub fn copy_ignored(dest: impl Display, commit: &str, detail: impl Display, record: impl Display) -> String {
    format!(
        "{dest} (commit {commit:.10}) has no line authorship: {detail}. The Copied-From trailer records its origin. \
         To remove this note, delete {record}"
    )
}

pub fn copy_unfinished(dest: impl Display, commit: &str, id: impl Display) -> String {
    format!("the copy check for {dest} (commit {commit:.10}) did not finish. Run it: prov copy-check {id}")
}

// The report.

pub fn check_ok(name: impl Display) -> String {
    format!("ok   {name}")
}

pub fn check_fail(name: impl Display) -> String {
    format!("FAIL {name}")
}

/// The first line of a problem, and each line after it.
pub const PROBLEM_FIRST: &str = "       - ";
pub const PROBLEM_NEXT: &str = "         ";

/// The first line of a warning, and each line after it.
pub const WARNING_FIRST: &str = "       warning: ";
pub const WARNING_NEXT: &str = "         ";

pub fn note(text: impl Display) -> String {
    format!("       ({text})")
}

pub const ALL_PASS: &str = "All checks pass.";
pub const SOME_FAIL: &str = "One check or more fails.";

// Commits with no git-ai note.

pub const NO_NOTES_REF: &str = "refs/notes/ai does not exist: git-ai has written no note in this repository yet";

pub fn await_failed(error: impl Display) -> String {
    format!("{error}. A commit of the last minute can show as a commit with no note.")
}

/// The kind of branch in [`gap_rewrite`] and [`gap`].
pub const HUMAN_BRANCH: &str = "a human branch";
pub const AGENT_BRANCH: &str = "an agent branch";

pub fn gap_accepted(commit: &str, subject: impl Display) -> String {
    format!("commit {commit:.10} ({subject}) has no git-ai note; known_gaps accepts it")
}

/// The last line of [`gap_rewrite`] and [`gap`].
pub fn gap_accept(commit: impl Display, file: impl Display) -> String {
    format!("Or accept the gap: add \"{commit}\" to known_gaps in {file}, and commit it.")
}

/// A line of [`gap_rewrite`] and [`gap`].
pub const GAP_REPLACE: &str = "Replace the authorship of its files: git rm and commit, then prov cp.";

pub fn gap_rewrite(commit: &str, kind: impl Display, subject: impl Display, old: &str, replace: impl Display, accept: impl Display) -> String {
    format!(
        "commit {commit:.10} on {kind} ({subject}) has no git-ai note. Commit {old:.10}, which no branch \
         contains, has the same patch and a note: an amend or a rebase made {commit:.10} while the git-ai \
         service did not see it.\nIf no one else has the branch, make the rewrite again from {old:.10} while \
         the service runs (`git-ai bg status`), so that git-ai moves the note.\nIf the branch is public: {replace}\n{accept}"
    )
}

pub fn gap(commit: &str, kind: impl Display, subject: impl Display, replace: impl Display, accept: impl Display) -> String {
    format!(
        "commit {commit:.10} on {kind} ({subject}) has no git-ai note: git-ai did not see the commit, and \
         no other commit with the same patch has a note.\n{replace}\n{accept}"
    )
}

pub fn stale_known_gap(gap: impl Display) -> String {
    format!("known_gaps names {gap}, which is not a commit with no note on a local branch. You can remove it.")
}

// The allow and exclude lists of git-ai. Also in `prov cp`.

pub fn excluded(url: impl Display) -> String {
    format!("the remote {url} is in git-ai's exclude_repositories. git-ai ignores checkpoints here.")
}

pub fn not_allowed(allow: impl Display, no_remote: bool) -> String {
    let no_remote = if no_remote { " (it has no remote)" } else { "" };
    format!(
        "git-ai's allow_repositories ({allow}) matches no remote of this repository{no_remote}. git-ai ignores checkpoints here. \
         Add a remote that matches, or: git-ai config --add allow_repositories <pattern>"
    )
}
