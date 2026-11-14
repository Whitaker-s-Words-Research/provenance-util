# Changelog

This file lists the changes to `prov` by date, oldest first. All changes so far belong to version 0.1.0, which is not released yet.

Each day lists major features, minor features, major bug fixes, and the other bug fixes in one line. A day with no bug fixes has no bug sections. Changes to the documentation and the repository setup are under minor features.

## 2026-11-11

### Major features

- The Cargo workspace, with four library crates and the `prov` binary.
- `prov-core`: the configuration, path patterns, live state, session states and commit trailers.
- `prov-git`: worktree discovery and the git commands that prov runs.
- `prov-adapters`: the adapter interface for agents, and the Claude Code adapter.
- `prov-gitai`: the git-ai commands that prov runs, and their JSON output.
- `prov`: the agent hooks and git hooks, the sweep, the archive step, the merge gate and `prov check`.

### Minor features

- The toolchain pin and the license.
- PLAN.md with the testing notes.

## 2026-11-12

### Major features

- Human branches. Every branch that is not an agent branch is a human branch. The `human_branch` config key is removed. A human can work on a feature branch with agent branches from it.
- Merge gate decisions:
  - `prov check` and `prov check <agent branch>` compare against the merge target: `--into`, or the branch that is checked out.
  - `prov check <base>..<tip>` follows the first-parent line of each human branch that a merge brings in.
  - The lines of a session that only a note names, and that has no record, are doubtful lines.
  - A path that a merge commit on the incoming branch changed relative to every parent needs an approval.
  - A stale session does not occupy its worktree. It must still be closed before its branch integrates.
- `prov init` writes the config, the shims, the agent settings and the git options. An existing hook stays as `<name>.local`, and the prov hooks run it.
- `prov doctor` runs eight checks of the setup.
- `prov start`, `prov finish` and `prov session` manage worktrees and session records. Every command that changes state has `--dry-run`. Every command that reads state has `--json`.
- `prov approve`: `--merge` writes the approval into the merge message. `--human` and `--ai` run a git-ai checkpoint and keep the approval in live state until the commit, where it becomes a trailer.
- `prov trace export` writes Agent Trace records and validates each one against the pinned Agent Trace 0.1.0 schema.
- `prov cp` copies files with their attribution. It builds a plan from git-ai blame, lets the human resolve it, and replays it with checkpoints. Each copied file gets a `Copied-From` trailer.
- The copy check. After the commit, a background check compares the git-ai note with the plan. `prov doctor` reports a file that failed and commits with no note.
- Rules from the TLA+ model review: taint of a worktree with a new occupant, a sweep at SessionEnd, session generations, an archive step that waits for the agent process and verifies an existing archive, and a gate item for files with no ranges.
- TLA+ models of the session states, the sweep and the integration, with `tla/check-models.sh`.
- Local checks in Podman: `ci/run.sh` checks a clean clone of HEAD, `ci/dev.sh` runs cargo with a cached build, and `ci/pre-push` runs the checks before a push. Hosted CI is removed.
- Tab completion for bash, zsh and fish. Branches and session IDs come from the repository of the current directory.

### Minor features

- The repository forces merge commits with `merge.ff = false`.
- Pending approvals of `prov approve --human` and `--ai` are kept in `.git/provenance/approvals/`.
- The Claude Code adapter merges its hooks, the agent identity and `commit.gpgsign=false` into `.claude/settings.json`, reports what the file lacks, and checks that the git-ai hooks are installed.
- `prov-gitai`: the `known_human` checkpoint, the git-ai version check, config lists, blame, a checkpoint before AI edits, and the tested version range.
- `prov-core`: copy plans, copy records, worktree records with the occupant, session generations, `Prov-Model` trailers and the `known_gaps` setting.
- Commands that end a session list the processes that still run in the worktree.
- `prov completions [--shell] [--install]` prints the line for the shell's startup file, or adds it once. `prov init` prints a hint when the line is missing.
- `prov start --path <dir>` names the directory that holds the worktree.
- `prov finish` prints the command that deletes the branch: `git branch -d` when the branch is merged, `-D` with a warning when not.
- The `commit-msg` shim is removed.
- Toolchain 1.100.0, with clippy `pedantic` for the workspace.
- `crates/prov/tests/gitai_contract.rs` checks each git-ai behavior that prov depends on.
- The README: background, status, installing and developing.
- PLAN.md is rewritten as a current-status document.
- User guides in `docs/`, and a shorter README.
- The repository uses `prov` on itself.

### Bug Fixes

Bug Fixes: clippy findings, a race in the git-ai service test, and `.gitignore` entries that hid new files in `crates/` and `vendor/` from git and from `ci/dev.sh`.

## 2026-11-13

### Major features

- `CLAUDE.md` and `AGENTS.md` are protected paths by default, at every level of the tree. Reason: an agent could weaken the controls by changing its own instruction files on its branch.
- All user-facing text is in the `prov-messages` crate. A snapshot test checks the help texts.
- `prov discard <session or branch>` deletes the work of an agent branch that will not be integrated: the worktree, the branch, the session records and the transcript archives. It lists what it will delete, and deletes only with `--yes`. It refuses while a session of the branch is open or closing, while a merge of the branch is in progress, and when another ref names one of its sessions or copied a file from one of its commits. The TLA+ model `tla/ProvDiscard` checks the order of its steps.
- `prov revise [--of <merge>] [--dry-run]` commits staged human changes to the lines of an integration, with a `Prov-Revises: <merge>` trailer. The staged diff must change or delete at least one line of the integration.

### Minor features

- `prov session list` and `prov session show` show the session title from the Claude Code transcript. `list` shows the shortest unique session ID prefix and the worktree path with `~`.
- AGENTS.md and CLAUDE.md hold the instructions for agents in this repository.
- The user guides, the README and the messages of prov no longer show rule IDs, TLA+ models or test evidence.
- The merge gate reads the session records under the lock.
- After an integration, post-commit and post-merge print a hint about `prov revise`.
- `prov session close` is now `prov session archive`. Reason: "close" was easy to confuse with `prov finish`.
- Every `--dry-run` and `--json` option has a help text.

### Major bug fixes

- `prov start --path` expands a leading `~`. Before, `--path=~/dir` or a quoted value created the worktree in `<repo>/~/...`.

### Bug Fixes

Bug Fixes: `ci/dev.sh` with `PROV_UPDATE_SNAPSHOTS` now copies the snapshot files back to the checkout; the waits of the merge gate for late notes share one limit of 60 seconds, instead of 60 seconds for each commit; a session record that does not parse is reported as such, not as a missing record.
