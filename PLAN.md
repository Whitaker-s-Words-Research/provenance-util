# prov: implementation plan

Written 2026-11-11. Nothing is implemented yet. The specification is [SPEC.md](SPEC.md); read it first. The user-facing overview is [README.md](README.md).

The work is split into three sessions. Start each in a new Claude Code session, in this directory.

| Session | Model | Scope | Why this model |
|---|---|---|---|
| 1. Core | Fable | Workspace, sessions, hooks, sweep, merge gate, `prov check` | These parts depend on each other, and a wrong design choice here is expensive. |
| 2. Commands | Opus (or Fable if the usage limit allows) | `init`, `doctor`, `start`, `finish`, `session`, archive step, `approve`, `trace export`, plumbing | They follow the spec closely and have little design left. |
| 3. Copy | Fable | `prov cp` | It is the largest part of the design that no test has touched. |

OpenCode is phase 2 and is not in this plan. See "Appendix: phase 2, OpenCode" in the spec. Do not make phase 1 rules specific to Claude Code; the appendix lists the six rules that must stay general.

## Session 1: core (Fable)

Build these together, in this order. Each row names the spec section to follow.

- [x] **Workspace.** Cargo workspace with the five crates from "Build and release": `prov-core`, `prov-git`, `prov-adapters`, `prov-gitai`, `prov`. Pinned toolchain in `rust-toolchain.toml`. License MIT.
- [x] **Test harness.** Integration tests that create temporary git repositories with worktrees and run the real `git` and `git-ai`. See "Tests" and the notes on testing below.
- [x] **Config and live state.** Read `.provenance.toml` (five keys, plus the optional `[agent.<name>]` section). Session records in `.git/provenance/sessions/`, with the file lock. See "Configuration" and "Live state".
- [x] **Session state machine.** Open, Closing, Archived, Failed. Stale is calculated, not stored. Contended worktrees. See "Session states" and "Binding rules".
- [x] **Claude Code hooks.** `prov hook claude-code <event>` for SessionStart, UserPromptSubmit, Stop and SessionEnd. Each must return in less than 100 ms at the 95th percentile. See "Agent hooks".
- [x] **Sweep.** The `agent-v1` checkpoint at each Stop and in `pre-commit`, with the session ID and the correct model. See "Keeping attribution complete" and "Agent model".
- [x] **Git hooks.** `prov hook git <name>` for `pre-commit`, `prepare-commit-msg`, `post-commit` and `post-merge`. See "Git hooks".
- [x] **Merge gate.** What counts as an integration, the sessions of an integration, doubtful lines, protected paths, the integration trailers, refusing a squash. See "Integration into a human branch" and "The merge gate".
- [x] **`prov check`.** The three forms: no argument, an agent branch, a range for CI. It shares the rules of the gate. See "Checking".

For session 1, a minimal archive step is enough to reach the Archived state (the gate needs a transcript hash). The full archive rules are in session 2.

Done when: an integration test runs a recorded Claude Code session in an agent worktree, merges the branch, sees the gate refuse a doubtful line, approves it by writing the trailer into the merge message by hand, completes the merge, and `prov check <base>..<tip>` passes.

**Status: done on 2026-11-11.** `cargo test --workspace` passes. The "done when" test is `crates/prov/tests/integration.rs`.

What session 1 left for session 2, beyond its list:

- The git hooks do not yet run `<name>.local`. (Done in session 2.)
- `prov approve` does not exist. The tests write `Prov-Approved` into `.git/MERGE_MSG` with `git interpret-trailers`. A trailer added by hand must be in the last paragraph of the message, in one block with the other trailers; git ignores a trailer in an earlier paragraph.
- The gate messages name `prov session close`, which does not exist yet.
- The archive step is complete (deterministic tar, hash, gzip, detached process, Failed state). Session 2 only has to call it from `prov session close`.
- `prov hook` is parsed by hand in `main.rs`, before clap. Keep it so: building the clap parser costs more than the rest of a hook.

## Session 2: commands (Opus, or Fable)

- [x] **`prov init`.** Config file, shims in the hooks directory, Claude Code settings, `merge.ff = false` and `pull.ff = only`. There is no `human_branch` key: every branch that does not match `agent/**` is a human branch, so the merge commit is forced for the whole repository, not for one branch. Keeps an existing hook as `<name>.local`. Stops if the hooks directory is inside a worktree.
- [x] **`prov doctor`.** The eight checks in "Doctor checks".
- [x] **`prov start`.** Worktree, branch name by convention, launch, `--resume`, `--no-launch`. `--from` defaults to the branch that is checked out, which can be a feature branch.
- [x] **`prov finish`.** `git worktree remove`, the refusal while a session is Open or Closing, `--override`, `--force`.
- [x] **`prov session`.** `list`, `show`, `close`, `drop`.
- [x] **Archive step, complete.** The deterministic tar with the transcript and the `<session ID>/` directory, SHA-256 before compression, the gzip file, the detached process, the Failed state. See "Archive step".
- [x] **`prov approve`.** `--merge` writes the trailer into `.git/MERGE_MSG`. `--human` and `--ai` run the checkpoints and keep the approval in live state until the commit. See "Approval".
- [x] **`prov trace export`.** Records from `git-ai diff --json`, validated against `vendor/agent-trace/0.1.0/`. See "Trace export".
- [x] **Plumbing.** `--json` and `--dry-run` on every command, the latency test, local checks in Podman (`ci/run.sh`, run by the pre-push hook `ci/pre-push`), release builds with `--locked`. Since 2026-11-12, no hosted CI: the GitHub workflows were removed.

Done when: every command in the "Commands" table except `prov cp` works, and `prov doctor` passes in a repository that `prov init` set up.

**Status: done on 2026-11-12, on the branch `session-2-commands`.** `cargo test --workspace --locked` passes. The tests of the commands are in `crates/prov/tests/commands.rs`.

Choices made in session 2 that the spec does not fix. Each is open for the user to change:

- **`prov approve` reads the typed `yes` from `/dev/tty`.** With no terminal it refuses. `--dry-run` shows the items with no prompt. The tests give it a pseudo-terminal (`Env::prov_tty`).
- **`prov session close` and `prov finish --override` run the archive step in the foreground**, not in a detached process. The command returns when the session is Archived or Failed.
- **`prov init` keeps an existing `.provenance.toml` that parses.** It replaces the file only with `--force`, and only when `--agent` asks for other agents or the file does not parse. It replaces a value in `.claude/settings.json` that differs (for example another `GIT_AUTHOR_EMAIL`) only with `--force`.
- **`prov doctor` decides "git-ai allows the repository" from `git-ai config allow_repositories` and `exclude_repositories`**, matched against the remote URLs. This repeats git-ai's rule; git-ai has no command that answers the question.
- **`prov trace export` finds the commits with a note through `git notes --ref=ai list`.** It reads only which commits have a note. The content comes from `git-ai diff --json`.
- **The content hash of a `--human` or `--ai` approval is the git blob ID** of the file. `prepare-commit-msg` compares it with the staged blob.
- **`prov start` is not refused for an agent.** The spec says "run by a human" and gives no refusal.
- **The JSON Schema validator is written in `crates/prov/src/schema.rs`**, for the keywords that the pinned schema uses. An unknown keyword is an error.

Not tested in session 2:

- `prov start` with the real `claude` command. The tests use a stub command from `[agent.claude]`. A real launch bills to the Claude Code login; ask first.
- A release script. Release builds are local now (no GitHub workflows); the steps for a tagged release with checksums are not written yet.
- macOS. Everything ran on Linux only.
- `prov doctor --deep` (`git-ai debug`).

What session 2 left for session 3:

- `prov doctor` reports a file in `.git/provenance/copies/` that has `"failed": true`. Session 3 defines that file.
- `prepare-commit-msg` adds the `Prov-Approved` trailers of `--human` and `--ai`. The `Copied-From` trailers and the copy guard in `pre-commit` are not there yet.

## Session 3: copy (Fable)

- [x] **Plan.** Build the plan from `git-ai blame --json` (only AI sessions are inherited), `--all`, `--missing`. TOML format with `author = "?"` for unresolved ranges.
- [x] **Resolving.** Editor flow, `--plan <file>`, `--apply <file>`, the printed summary.
- [x] **Replay.** Staged writes with `known_human` and `agent-v1` checkpoints. Directories, binary files, symbolic links, the executable bit.
- [x] **Guard and trailers.** Copy guard in `pre-commit`, one `Copied-From` trailer for each file.
- [x] **Deferred check.** Compare the note with the plan after the commit; report through `prov doctor`.
- [x] **Recipe test.** The steps in "Replacing the authorship of a file" work as written.

Done when: a test copies a directory with files of mixed authorship into one commit and each file agrees with its plan.

**Status of session 3: done on 2026-11-12, not committed, on the branch `session-3-copy`.** The tests are in `crates/prov/tests/copy.rs`. Not tested: an editor that needs the terminal (the test editor is a script), a source repository with a Git LFS or clean filter, and a copy with thousands of files.

## Spec rules from the model review

SPEC.md changed on 2026-11-12, after a review with TLA+ models (`tla/`, `SPEC-proposed-changes.md`). The code has these rules since the same day, not committed:

- [x] **R-SWEEP-2.** Taint: `worktrees/<key>.json` with the owner; every hook in an agent worktree looks at the occupants first; a change of the occupant gives the changed files an AI checkpoint under the tool `prov-unattributed`, with a new conversation ID. The first design (skip tainted files in the sweep) failed its test: git-ai gives a file with no checkpoint to an AI session of the same commit.
- [x] **R-SWEEP-5.** Sweep at SessionEnd, before the state changes.
- [x] **R-SESS-1, R-SESS-2.** A generation in each session record. The archive process writes only for its generation.
- [x] **R-SESS-3.** The archive step waits for the agent process if its ID is known. `prov session close` prints the process ID it waits for.
- [x] **R-ARCH-1.** An archive file that exists is verified. A unit test covers what a killed process leaves; no test kills a real process.
- [x] **R-INT-2.** `post-commit` deletes a named record only if it is Archived with the hash of the commit's trailer group.
- [x] ~~**R-INT-3.** The `commit-msg` shim.~~ Removed 2026-11-12 by the owner's decision, to trim the state graph: one gate run per merge. An incomplete merge message is an accepted limit that `prov check` reports.
- [x] **Trims of 2026-11-12** (owner's decision): `post-commit` deletes only the records that the merge's trailers name; `prov doctor` lists a record whose branch is merged. The 5-minute rule is calculated when read, never stored.
- [x] **Gate item for a file with no ranges** (paths that git-ai ignores), in the gate and in `prov check`.
- [x] **Model checks** run in the local checks (`ci/run.sh`), with tla2tools pinned by checksum in `ci/Containerfile`.

Tested with two live `claude -p` runs on 2026-11-12: `CLAUDE_PID` in a real hook is the agent process; a real launch and resume through `prov start`; the integration of the result. Not tested: a resume of a real session between the gate and `post-commit`.

Found in the live run: when every edit goes through the agent's own git-ai hooks, the note has the model `unknown`. git-ai takes a model only from a checkpoint that records a change, and the agent's hooks give none. Decided 2026-11-12: a `Prov-Model` trailer in each integration group, used by trace export (done), No issue for git-ai: the owner decided not to file one. In a second live run, git-ai's own hooks recorded the model; why the two runs differ is not known.

## How a design decision is made

Since 2026-11-12, after the first taint design failed (it rested on an untested assumption about git-ai):

1. **Write the rule in the spec, and model it in TLA+ if it depends on ordering.** Each model lists its premises: what it takes as a fact about git, git-ai or Claude Code. Each premise names a row of "Appendix: test evidence" in the spec, or says "untested".
2. **Test each premise with the real tools before the decision goes to the owner.** If a premise cannot be tested yet, the question to the owner says so.
3. **When a premise turns out false, search the spec for every other rule that rests on it**, and correct them in the same change.

A model checks the order of events. It cannot check a premise.

## Notes on testing

These come from tests during the design work. The evidence is in "Appendix: test evidence" in the spec.

- **Build and test in the container, not on the host.** The checkout is on a 9p mount, where a build and the full suite take minutes. `ci/dev.sh <cargo arguments>` runs any cargo command in the image of the local checks, on the working tree as it is, with the build cached in a Podman volume: the full suite takes about 30 seconds, clippy about 12. `ci/dev.sh --models` checks the TLA+ models. `ci/run.sh` is the clean run before a push (about 95 seconds). The exception is a test of `prov` itself on a slow file system: `PROV_TEST_REPOS=<dir on the mount> cargo test` on the host.
- **The containers run with `--init`.** Without an init process, a stopped git-ai service stays in `/proc` as a zombie, and a test that waits for it to exit fails.

- **Run git-ai tests outside an agent session, with a clean environment** (`env -i HOME=... PATH=...`). Inside a Claude Code session, git-ai's own hooks attribute changes made by test commands to that session. This happened once during the design work.
- **A test repository needs a remote that git-ai allows.** On this machine, `allow_repositories` is `github.com/Whitaker-s-Words-Research/*`. A remote URL under that name is enough; nothing is pushed.
- **git-ai writes a note 1 to 6 seconds after a commit.** Run `git-ai await --timeout 60` before reading it.
- **The git-ai service must run.** Check with `git-ai bg status --repo <dir>`.
- **Inside an agent session, give the tests their own git-ai service.** The harness in `crates/prov/tests/common/mod.rs` does this: a new `HOME` in `/tmp` with its own `.gitconfig` (the `trace2.eventTarget` socket of that home) and `git-ai bg start`. The agent's git-ai hooks then cannot reach the test repositories, and no allowed remote is necessary, because the new home has no `allow_repositories`. The home must be a short path: git-ai moves its sockets to `/tmp/git-ai-d-*` when the path is too long for a Unix socket, and the trace2 target is then wrong.
- **Future projects are developed on a 9p mount, so `prov` must work there.** Run the suite on it before a release: `PROV_TEST_REPOS="$PWD/target/p9" cargo test --workspace --locked`. It takes about 15 minutes there, against 1 minute in `/tmp`. Every test passes; the latency test uses a limit relative to the cost of git on that file system. Keep `prov` free of extra git processes and of walks over the worktree: each git process costs 25 to 100 ms there, and each file that git examines 2 ms. A large repository on such a mount makes every sweep slow (about 2 ms for each tracked file); `prov` cannot change that.
- **Run the binary from a local disk when you measure time.** This checkout is on a 9p mount. The start of any binary from it takes about 40 ms. The harness copies `prov` to `/tmp` first. From there, the agent hooks take 4 to 10 ms (median).
- **`git-ai bg start` does nothing if it runs while the old service still exits.** `git-ai bg shutdown` returns before the process is gone. A test that stops and starts the service must wait for the old process (`Env::stop_service` in the harness).
- **With `merge.ff = false`, git itself refuses `git merge --squash`** ("cannot be used together"). A squash needs `--squash --ff`. The refusal in `prepare-commit-msg` covers that case.
- **Tested versions:** git 2.53, git-ai 1.7.6, Claude Code 2.1.322, Rust 1.100.0 (rustup, pinned in `rust-toolchain.toml`; clippy runs in the local checks with `-D warnings`, lints in `[workspace.lints]`). The local checks use Ubuntu 26.04 in Podman, with the same git and git-ai versions. Check the appendix results again if a version changes. For git-ai: run `cargo test --test gitai_contract` against the new version, then raise `prov_gitai::TESTED_VERSION`.

## Constraints from the user

- **No API costs outside Claude Code.** Before a test calls a model, say what it bills to. Short `claude -p` runs use the Claude Code login and are acceptable in small numbers. Do not run other agents or API calls without asking.
- **Design choices go to the user as questions**, with a recommendation, not as direct edits to the spec.
- **A human owns the responsibility for what goes into the human branch.** Integration through a forge's merge button is never to be supported.
- **Nothing in this repository is committed yet.** Ask before the first commit, and before any push.

## Open questions in the spec

1. Where is the `prov` repository published?
2. ~~How does an untracked line appear in `git-ai diff --json`?~~ Answered 2026-11-12: in a commit by a human that also has lines of an AI session, a line with no checkpoint is an addition with no `session_id` and no `human_id`. See the table in "Keeping attribution complete".

## Parts of the design that no test has touched

Expect these to need changes when they are built. Raise them with the user.

- The copy plan and its editor flow (session 3).
- Running an existing hook as `<name>.local` (session 2).
- Refusing a local squash in `prepare-commit-msg` (session 1). Now tested with real git; it works. It reads the squashed commits from `.git/SQUASH_MSG`.
- Contended worktrees with two real sessions (session 1). Tested with recorded hook payloads only, not with two real Claude Code processes.
