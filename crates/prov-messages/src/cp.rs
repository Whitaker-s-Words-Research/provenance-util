//! `prov cp`.

use std::fmt::Display;

pub fn author_option(name: impl Display, value: impl Display) -> String {
    format!("{name} {value}: use `human` or `ai`")
}

pub const SOURCE_PATH_NOT_UTF8: &str = "a path of the source is not UTF-8";
pub const UNEXPECTED_LS_TREE: &str = "unexpected output of git ls-tree";

pub fn submodule(path: impl Display) -> String {
    format!("{path} is a submodule in the source. `prov cp` copies files only.")
}

pub fn lfs_not_installed(path: impl Display) -> String {
    format!(
        "{path} is a Git LFS pointer in the source, and git-lfs is not installed. Install git-lfs to copy its \
         content. Nothing was copied."
    )
}

pub fn lfs_object_missing(path: impl Display, source: impl Display) -> String {
    format!(
        "{path} is a Git LFS pointer, and its content is not in the local LFS store of {source}. Fetch it there \
         first: git lfs fetch, or git lfs pull. Nothing was copied."
    )
}

pub fn lfs_failed(path: impl Display, error: impl Display) -> String {
    format!("cannot read the Git LFS content of {path}: {error}. Nothing was copied.")
}

pub fn cannot_read_source(path: impl Display, commit: impl Display, source: impl Display) -> String {
    format!("cannot read {path} at {commit} in {source}")
}

pub fn not_in_worktree(dest: impl Display, worktree: impl Display) -> String {
    format!("{dest} is not in the worktree {worktree}")
}

pub fn not_utf8(dest: impl Display) -> String {
    format!("{dest} is not UTF-8")
}

pub const SOURCE_NOT_UTF8: &str = "the path of the source is not UTF-8";

pub fn git_failed(args: impl std::fmt::Debug, stderr: impl Display) -> String {
    format!("git {args:?} failed: {stderr}")
}

pub const NO_PATHS: &str = "give the source paths and the destination";
pub const NO_SOURCE_PATHS: &str = "give one source path or more, and then the destination";

pub fn absolute_source(source: impl Display) -> String {
    format!("{source}: a source path is relative to the root of the source repository")
}

pub fn no_source_file(commit: impl Display, source: impl Display, path: impl Display) -> String {
    format!("the commit {commit} of {source} has no file at `{path}`")
}

pub fn no_notes(source: impl Display) -> String {
    format!("the source {source} has no refs/notes/ai: no line has an author from the source")
}

pub fn plan_file_missing(commit: impl Display, path: impl Display) -> String {
    format!("the commit {commit} of the source has no file `{path}`")
}

pub fn kind_mismatch(path: impl Display, source_kind: impl Display, plan_kind: impl Display) -> String {
    format!("{path}: the file is {source_kind} in the source, and the plan says {plan_kind}")
}

/// The kinds of file in [`kind_mismatch`].
pub const KIND_SYMLINK: &str = "a symbolic link (symlink = true)";
pub const KIND_BINARY: &str = "a binary file (binary = true)";
pub const KIND_TEXT: &str = "a text file";

pub const TOO_MANY_LINES: &str = "a file has too many lines";

pub fn bad_dest(dest: impl Display) -> String {
    format!("`{dest}` is not a path in this worktree")
}

pub fn identical_lines(dest: impl Display, groups: impl Display) -> String {
    format!(
        "{dest}: {groups} group(s) of identical lines have different authors. The authors can move between \
         identical lines; the content and the counts stay correct."
    )
}

// The summary: one line for each file.

pub fn summary_symlink(dest: impl Display) -> String {
    format!("{dest}  symbolic link")
}

pub fn summary_binary(dest: impl Display) -> String {
    format!("{dest}  binary")
}

pub fn author_count(who: impl Display, count: impl Display) -> String {
    format!("{who}: {count}")
}

pub fn summary_text(dest: impl Display, lines: impl Display, authors: impl Display) -> String {
    format!("{dest}  {lines} line(s)  {authors}")
}

/// A file with no lines has no authors.
pub const EMPTY: &str = "empty";

/// One range of a file with more than one author. `lines` is padded to 9 characters.
pub fn summary_range(lines: impl Display, who: impl Display) -> String {
    format!("    {lines:9} {who}")
}

pub fn cannot_run_editor(editor: impl Display) -> String {
    format!("cannot run the editor `{editor}`")
}

pub fn editor_failed(editor: impl Display, file: impl Display) -> String {
    format!("the editor `{editor}` failed. The plan is in {file}")
}

/// The head of the plan file in the editor.
pub const EDIT_HELP: &str = "\
# Give each range with author = \"?\" an author, then save and close.
#   { lines = \"1-4\", author = \"human\" }
#   { lines = \"5-9\", author = \"ai\", agent = \"claude\", model = \"m1\", session = \"s1\" }
# `agent` and `model` are optional (default: unknown). `session` is optional.
# You can split a range into more ranges. The ranges of a file cover every line once.
# A human range gets you as its author, also for a line that another human wrote.
# To stop, close the editor with a range still at \"?\".

";

pub fn agent_env(var: impl Display) -> String {
    format!("refused: only a human can run `prov cp`. {var} is set: an agent started this command.")
}

pub const AGENT_WORKTREE: &str =
    "refused: `prov cp` does not run in an agent worktree. Copy where you commit, on a human branch.";
pub const ALL_AND_MISSING: &str = "--all sets every line, so --missing has no effect. Give one of them.";
pub const AGENT_MODEL_WITHOUT_AI: &str = "--agent and --model go with `--all ai` or `--missing ai`";
pub const SESSION_WITHOUT_ALL_AI: &str = "--session goes with `--all ai`";
pub const PLAN_AND_APPLY: &str = "--plan writes a plan and --apply applies one. Give one of them.";
pub const APPLY_ONLY_FILE: &str =
    "--apply takes the source, the paths and the authors from the plan file. Give only the file.";

pub fn plan_source_not_repo(repo: impl Display) -> String {
    format!("the source `{repo}` of the plan is not a git repository")
}

pub fn no_source_commit(commit: impl Display) -> String {
    format!("the source has no commit `{commit}`")
}

pub const NO_FROM: &str = "give the source repository: --from <repo>";

pub fn from_not_repo(path: impl Display) -> String {
    format!("{path} is not a git repository. `--from` takes the path of a local clone.")
}

pub const EMPTY_PLAN: &str = "the plan has no file";
pub const DRY_RUN: &str = "Dry run: nothing changed.";

pub fn plan_written(file: impl Display) -> String {
    format!("The plan is in {file}. Apply it: prov cp --apply {file}")
}

pub const PLAN_NO_DIR: &str = "the plan file has no directory";
pub const SOURCE_CHANGED: &str = "the [source] of the plan changed. To copy from another commit, run `prov cp` again.";

pub fn plan_invalid(error: impl Display, file: impl Display) -> String {
    format!("{error}\nYour plan is in {file}. Correct it, then: prov cp --apply {file}")
}

pub fn unresolved_count(count: impl Display) -> String {
    format!("{count} range(s) have no author:")
}

pub fn unresolved_item(name: impl Display) -> String {
    format!("  {name}")
}

pub fn set_authors(file: impl Display) -> String {
    format!("Nothing is written to the working tree. Set the authors in {file}, then: prov cp --apply {file}")
}

pub const GIVE_AUTHORS: &str =
    "Nothing is written to the working tree. Give --missing or --all, or write a plan with --plan <file>.";

pub fn dest_exists(dest: impl Display) -> String {
    format!(
        "the destination {dest} exists. `prov cp` does not replace a file. To replace the authorship of a file: \
         git rm it and commit, then copy it again."
    )
}

pub fn parent_not_dir(dir: impl Display, dest: impl Display) -> String {
    format!("{dir} is not a directory, so {dest} cannot be written")
}

pub fn copy_stopped(dest: impl Display) -> String {
    format!("the copy of {dest} stopped")
}

pub fn filter_changed(dest: impl Display) -> String {
    format!("{dest}: a filter of this repository changed the bytes that git stores")
}

pub fn copied(count: impl Display) -> String {
    format!("Copied and staged {count} file(s). Commit them: git commit")
}

pub const TRAILER_NOTE: &str =
    "The commit gets one Copied-From trailer for each file. A change to a copied file before the commit is refused.";

/// A range with no author, in the list after [`unresolved_count`] and in the JSON output.
pub fn unresolved_line(dest: impl Display, line: impl Display) -> String {
    format!("{dest} line {line}")
}

pub fn unresolved_lines(dest: impl Display, start: impl Display, end: impl Display) -> String {
    format!("{dest} lines {start}-{end}")
}

pub fn note(text: impl Display) -> String {
    format!("note: {text}")
}

pub fn cannot_create_symlink(path: impl Display) -> String {
    format!("cannot create the symbolic link {path}")
}

pub fn not_equal(dest: impl Display) -> String {
    format!("{dest} is not equal to the source after the last stage")
}
