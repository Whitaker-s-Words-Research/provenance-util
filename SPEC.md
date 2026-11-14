# prov: specification

Version 0.1 (draft). License: MIT.

`prov` is a command-line tool for git repositories where AI agents and humans both make commits. It does these jobs:

1. It gives each agent session its own worktree and branch.
2. It records which session made which commits, with a hash of each session transcript.
3. It controls what reaches a human branch.
4. It sets and reads line authorship through git-ai.
5. It copies files from another repository with their line authorship.
6. It exports authorship as Agent Trace records (optional).

## Motivation

Provenance tooling built inside a project tends to have three faults:

- It mixes the tooling's own history into the project that it describes.
- It spreads session state across scripts that each own a part of it.
- It assumes one agent session at a time.

`prov` is a new design that avoids all three.

## Non-goals

Not in version 1:

- A web interface. `prov` is a command-line tool only. Its `--json` output is for scripts.
- Calls to an LLM API.
- Project checks such as test markers. These stay in each project.
- Agents other than Claude Code. OpenCode is planned for phase 2; see "Appendix: phase 2, OpenCode". The adapter interface must permit it and more agents later.
- Compatibility with earlier provenance scripts or their data.

Not in any version:

- Integration of an agent branch through the merge button of a forge, for example a GitHub pull request. Reason: a human owns the responsibility for what goes into a human branch. The human does the integration locally, where the merge gate runs. `prov check` reports a forge merge as a failure. Protect the human branches on the forge so that agent branches cannot be merged on the forge.

## Terms

| Term | Meaning |
|---|---|
| Human | A person who commits to the repository. A repository can have more than one. "The human" in this specification is the person who runs the command. |
| Human branch | A branch that does not match `agent/**`. Only humans commit to it. Examples: `main`, and a feature branch that a human works on. An agent branch can start from any human branch and integrate into it. |
| Human worktree | A worktree that has a human branch checked out. |
| Base directory | The directory that holds the common git directory: the repository root in a normal clone, and the container directory of a bare repository with linked worktrees. |
| Hooks directory | The directory where git looks for hooks: `git rev-parse --git-path hooks`. |
| Agent branch | A branch where one or more agent sessions commit. Its name matches `agent/**`. |
| Agent worktree | A git worktree on an agent branch. It holds one occupant at a time. |
| Session | One run of an agent, identified by the agent's session ID. |
| Occupant | An Open session of an agent worktree that is not stale. A worktree has no occupant, one occupant, or more than one. |
| Contended worktree | An agent worktree with more than one occupant. See "Binding rules". |
| Human commit | A commit whose author email is not an agent email from the config. A commit with a `Prov-Session` trailer is a human commit only if it also has a `Prov-Version` trailer, which marks an integration. |
| Integration | The merge commit that brings agent commits into a human branch. See "Integration into a human branch". |
| Incoming range | The commits that an integration brings in: `HEAD..<incoming tip>`. |
| Agent commit | In an integration: every commit in the incoming range, whatever its author. |
| Note | The git-ai authorship record on a commit, in `refs/notes/ai`, format `authorship/3.0.0`. |
| Checkpoint | A git-ai command that tells git-ai who changed a file. git-ai writes the note at commit time. |
| Doubtful line | A line whose authorship the human must approve before integration. See "The merge gate". |
| Plan | The list of files and line ranges of a copy, with the author of each range. See "Copying files". |
| Live state | Files in `.git/provenance/` that `prov` keeps while work is in progress. |

## Rules for all commands

- `prov` changes git state only through the `git` command. It writes inside `.git` only in `.git/provenance/`, the hook shims in the hooks directory, and a commit message that git is preparing: the file that git gives to `prepare-commit-msg`, or `.git/MERGE_MSG` during a merge. It changes a message only through `git interpret-trailers`. Reason: the gate and `prov approve --merge` record their results as trailers of the merge message (owner decision, 2026-11-13).
- `prov` never writes git-ai data. It sets authorship only through public git-ai checkpoints, and git-ai writes the note.
- `prov` reads authorship only through the JSON output of git-ai: `git-ai diff <commit> --json`, `git-ai blame <file> --json` and `git-ai stats <commit> --json`. It has no parser for the note format. Before it reads the note of a new commit, it runs `git-ai await --timeout 60`.
    - **Reason**: git-ai writes a note 1 to 6 seconds after the commit, after the commit hooks finish.
- Every command that changes state accepts `--dry-run`, except `prov hook`. Every command that reads state accepts `--json`.
- A fact is stored in one place only. git-ai notes hold line authorship. Commit trailers hold session records. Live state holds only work in progress.
- Path patterns in the config, and the branch pattern `agent/**`, follow gitignore rules. `*` does not match `/`. `**` matches across directories.
- Relative paths in the config resolve against the base directory.

## Requirements

- One self-contained binary, written in Rust. It needs only `git` and git-ai at run time.
- It runs on Linux and macOS. Static linking is not necessary.
- Hook shims and generated scripts are POSIX `sh`. `prov` does hashing, compression and process detaching itself. It does not call `sha256sum`, `gzip`, `setsid` or `nohup`.
- An agent hook returns in less than 100 ms at the 95th percentile, when the worktree is on a local file system.
- On a slow file system, for example a 9p or network mount, the limit cannot hold: git needs about 2 ms for each file that it examines, and `git status` alone then takes longer than 100 ms. There, a hook adds no more than 100 ms to three times the cost of the git commands that it must run (`git rev-parse` and, for a sweep, `git status`). `prov` sets no timeout of its own in a hook. If the agent stops a hook at its own time limit, the sweep does not happen, and the changes become doubtful lines.
- git-ai must allow the repository. An empty `allow_repositories` (the default) allows every repository, also one with no remote. Once the list has an entry, git-ai matches it against remote URLs only: a repository with no remote is then not allowed, even by `*`, and a path entry needs a remote. A repository that matches `exclude_repositories` is not allowed either. In a repository that is not allowed, git-ai ignores checkpoints and prints only a message. A remote URL that matches is enough; the remote need not exist.
- A session is integrated from the clone that ran it. Reason: live state and transcripts are local to a clone.
- Minimum git version: 2.31. Reason: `GIT_CONFIG_COUNT`.
- Compatibility baseline: git-ai 1.7.6. `git-ai await` is marked beta in that version, and `prov` depends on it.
- Supported git-ai range: from the baseline to the newest version that the git-ai contract tests passed against (the tested version, 1.7.6). `prov` depends on how git-ai attributes lines, and a change there makes the gate show wrong authors with no error. A version newer than the tested version is not refused: `prov doctor` warns (owner decision, 2026-11-12). The tested version is raised only after the contract tests pass against the new version.
- The hook events in this specification were tested with Claude Code 2.1.322.

## Sessions, worktrees and branches

Each agent session works in its own worktree, never in a human worktree. Reason: parallel sessions need separate files and branches, and the sweep is valid only where nothing else changes files.

### Starting a session

`prov start <agent>` creates a worktree on a new agent branch and starts the agent in that worktree. This command is optional. Only a human runs it: it refuses when an agent environment variable is set (owner decision, 2026-11-13). Reason: an agent must not open sessions and worktrees by itself. A person can also run `git worktree add` and start the agent. The agent's SessionStart hook binds the session to the worktree in both cases.

```text
prov start <agent> [options] [-- <agent arguments>]
```

| Option | Default | Effect |
|---|---|---|
| `--name <slug>` | 4 random hex characters | The `<name>` part of the branch name. Letters, digits and `-` only. |
| `--branch <name>` | `agent/<agent>/<YYYYMMDD>-<name>` | Full branch name. It must match `agent/**`. |
| `--from <ref>` | the branch that is checked out | Start point of the new branch. |
| `--path <dir>` | `prov.worktreesDir` from the git config of the clone, or `../<repo>.worktrees/`, from the base directory | The directory that holds the worktree. `<repo>` is the name of the base directory. The worktree is the directory `<dir>/<branch>`, with the leading `agent/` removed and each `/` replaced by `-`: `agent/claude/20261112-71fe` gives `claude-20261112-71fe`. A leading `~` is the home directory. Reason: the shell does not expand `~` in `--path=~/dir`. With `--resume`, this applies when `--path` is given or the session has no record. |
| `--no-launch` | off | Create the worktree, print its path, and do not start the agent. Use this for agents in an IDE. |
| `--resume <session>` | none | Open the worktree of that session and start the agent with its resume option. If the worktree was removed, `prov` creates it again on the session's branch. If the session has no record, also give `--branch`. |

`prov` names the branch, not the session. Reason: the agent assigns the session ID at SessionStart, after the branch exists. The branch and worktree names are fixed conventions, not config keys. The directory that holds the worktrees can be set for one clone with `git config prov.worktreesDir <dir>`. A relative value resolves against the base directory. It is a git config key, not a key of `.provenance.toml`. Reason: the right place depends on the machine, for example a local disk when the repository is on a slow mount, and `.provenance.toml` is shared by every clone (owner decision, 2026-11-13). The date is the local date. It makes `git branch --list 'agent/*'` sort by time. If the branch exists, `prov start` stops with an error.

### Binding rules

- An agent worktree holds one Open session at a time. Only an Open session that is not stale occupies a worktree. A Closing session does not. A stale session does not: a new session in its worktree works normally. The stale session stays Open, and it must be closed before its branch integrates.
- After a session ends, a new session can bind to the same worktree and branch.
- `prov` cannot stop a second session. Reason: a SessionStart hook cannot stop Claude Code, and not every agent has a start event. When a second session starts in an occupied worktree, the worktree becomes contended:
    - `prov` records the second session as Open.
    - For Claude Code, the SessionStart hook prints an instruction to stop. Claude Code adds that text to the session context.
    - **[R-SWEEP-3]** `prov` runs no sweep in a contended worktree, and `pre-commit` refuses every commit there.
    - The worktree stays contended until only one session occupies it. A session ends by itself, or the human runs `prov session archive`.
    - Changes from that time are tainted (R-SWEEP-2), so they become doubtful lines. `prov` does not attribute them to one of the sessions.
- A session finds its record by its worktree. The agent's own environment variable is a cross-check. Claude Code sets `CLAUDE_CODE_SESSION_ID`.

### Sessions outside agent worktrees

`prov` records no session in a human worktree, or in a worktree on a branch that does not match `agent/**`. It has no transcript archive and no sweep.

- `prov` shows a warning. For Claude Code, the SessionStart hook prints it, and Claude Code adds it to the session context.
- `pre-commit` refuses a commit by an agent on such a branch.
- A human can still commit the changes of such a session. `prov check` then fails in CI, because the note of a human commit names an AI session. See "Checking".

Agent work reaches a human branch only through an integration.

### Session states

| State | Entered when |
|---|---|
| Open | The first event records the session. **[R-SESS-1]** Any later agent event of a session that is Closing, Archived or Failed also returns it to Open, whether or not it is a resume. SessionEnd is the exception: it never reopens a session. Reason: an event shows that the agent still works, for example after `prov session archive`; a session that stayed Closing would be archived without that work. |
| Closing | SessionEnd occurs, or the human runs `prov session archive`. A detached process starts the archive step. |
| Archived | The archive step stores the transcript. |
| Failed | The archive step cannot finish. The record holds the error text. A session that is Closing for 5 minutes with no live archive process is also Failed. That case is calculated when the state is read, like stale; it is not stored, so no process writes it. |

- An Open session is stale when no agent event arrives for 24 hours, or when the agent process does not exist. Stale is calculated when the state is read. It is not stored.
- To find the agent process, `prov` uses `CLAUDE_PID` for Claude Code and the parent process for other agents.
- Each record has a generation. It increases by 1 each time the session enters Open or Closing.
- `prov session archive <id>` moves an Open session to Closing. It runs the archive step in the foreground, as `prov finish --override` does: the command waits and shows the result. Reason: a person who runs it wants to see whether the archive succeeded (owner decision, 2026-11-13). SessionEnd starts a detached process instead, because the agent gives the hook little time. For a Failed session, it runs the archive step again. If the agent process still runs and `prov` knows its process ID, the session stays Closing until the process exits (R-SESS-3). `prov session archive` then prints the process ID that it waits for. If `prov` does not know the process ID, `prov session archive` and `prov finish` print a warning that names the accepted limit of R-SESS-3.
- `prov session drop <id>` deletes the record of a session. It keeps the transcript archive.
- `prov doctor` reports stale and Failed sessions.
- An integration deletes the records of the sessions on its branch. See "Sessions of an integration".

### Resuming

A resumed Claude Code session keeps its session ID. A session in any state can be resumed. It returns to Open. A session that is already integrated has no record. When it is resumed, the first event makes a new record with the same session ID. Each archive of a session is a separate file, so the hash in an earlier integration stays correct.

### Agent model

The sweep checkpoint needs the model of the session. The adapter finds it, and `prov` keeps the latest model in the session's live state:

- Claude Code: the `model` field of the SessionStart payload. At each Stop, and in `pre-commit` before the sweep, the model of the latest assistant message in the transcript replaces it. Reason for `pre-commit`: an agent can commit before its first Stop, and with `claude -p` the record had no model from SessionStart. The assistant message that runs `git commit` is in the transcript before the command runs.
- No known model: `unknown`.

The sweep checkpoint joins the git-ai session of the agent's own hooks, because the tool name and the conversation ID are the same. A git-ai session has one model, and the model of the sweep can become the model of the whole session. In a test with Claude Code, the agent's own hooks recorded the model `unknown`, and the note then had the model that the sweep gave. `prov` must therefore give the correct model.

git-ai takes a model only from a checkpoint that records a change. When every edit of a session goes through the agent's own git-ai hooks, the sweep has nothing to record, and the note has the model that those hooks gave: `unknown` in one live run, the right model in another. No public git-ai command sets the model alone. So the integration records the model of each session in a `Prov-Model` trailer, and trace export uses it where the note has `unknown`.

### Archive step

**[R-SESS-2]** An archive process belongs to one generation of its session. It changes the state, to Archived or to Failed, only under the lock and only if the session is still Closing with that generation. Otherwise it stores its file and changes nothing in the record. The 5-minute rule in "Session states" needs no such condition: it is calculated when read and never written. Reason: without the generation, this fails: a session ends, is resumed, does more work and ends again, and the first archive process then sets Archived with a transcript that does not cover the new work.

0. **[R-SESS-3]** Wait until the agent process has exited, if `prov` knows its process ID. While the process runs, the session stays Closing, and the archive process counts as live for the 5-minute rule. If the agent sends an event meanwhile, the session returns to Open (R-SESS-1), and this process ends without a change (R-SESS-2). If `prov` does not know the process ID, the step does not wait. This is an accepted limit: a session that the human closes while its agent works can then be Archived with a transcript that does not cover the last work, until the next event of the agent returns it to Open. The model `ProvSessions.limit-no-pid` records it. Reason for the wait: `prov session archive` and `prov finish --override` can close a session while its agent still works, and an archive made then misses the agent's last work (`ProvSessions.bug-no-wait`).
1. Build the archive through the adapter. The adapter decides how it gets the transcript. The archive is a tar file that `prov` builds:
    - Claude Code: the file at the transcript path from the hook payload, as `transcript.jsonl`, and every file of the directory `<session ID>/` next to the transcript, with its relative path. Reason: Claude Code stores large tool results in `<session ID>/tool-results/` and the messages of subagents in `<session ID>/subagents/`, not in the transcript.
    - The entries are sorted by name, and each entry has modification time 0, owner 0 and mode 0644, so the same files always give the same bytes. `tar -tv` therefore shows each entry with the date 1970-01-01 (owner decision, 2026-11-12: kept). A real time would make two archives of the same transcript differ, and R-ARCH-1 would then fail a retry.
2. Calculate SHA-256 of the tar bytes, before compression. The hash therefore covers the transcript, the tool results and the subagent sessions. To check an archive: decompress it and hash the result.
3. **[R-ARCH-1]** Write the archive, gzip-compressed, to a temporary file in the same directory, flush it, and give it the name `<logs_dir>/<agent>/<session>-<first 12 hex characters of the hash>.tar.gz` with an operation that fails if the name exists. If the name exists and its decompressed content has the same hash, the step succeeds. If it exists with another hash, the step fails. `prov` never overwrites an archive file. Reason: a process that stops during the write would otherwise leave part of a file under the final name, and every later attempt would fail.
4. Set the state to Archived. If a step fails, set the state to Failed and store the error text.

Every session that ends is archived, also a session that made no commit.

### Finishing

`prov finish <session or branch>` runs `git worktree remove` and keeps the branch. The branch is a record of the session.

- `prov finish` refuses while a session in the worktree is Open or Closing.
- `--override` removes the worktree in that case too. It first moves each Open session of the worktree to Closing.
- `--force` goes to `git worktree remove`.
- `prov finish` warns about processes that still run in the worktree (R-SWEEP-4). It does not refuse.
- `prov finish` keeps the session records. Reason: a later integration of the branch needs the transcript hash. The records are deleted only by the integration or by `prov session drop`.
- `prov finish` never deletes a branch. Reason: `git branch -d` checks the branch's upstream, not the human branch that it started from, so it can delete a branch that is not integrated. Only `prov discard` deletes a branch, after its own checks (see "Discarding").
- `prov finish` prints a command that deletes the branch, and does not run it. If the branch is merged into `HEAD` of the worktree where `prov finish` runs, the command is `git branch -d <branch>`. Otherwise it is `git branch -D <branch>`, and `prov finish` says that the branch is not merged. `--json` gives `merged` and `delete_command`.

### Discarding

`prov discard <session or branch>` deletes the work of an agent branch that will not be integrated: its worktree, the branch, the session records and the transcript archives. Reason: by hand this takes four commands (`prov finish --force`, `git branch -D`, `prov session drop` for each session, `rm` of each archive), and each one can delete evidence that an integration needs.

Terms in this section:

- The branch: the agent branch that is given, or the branch in the record of the session that is given. It must match `agent/**`.
- Other refs: every ref except the branch and its remote-tracking branches.
- The branch's own commits: the commits reachable from the branch and from no other ref.
- The sessions of the discard: each session whose record names the branch, and each session that a `Prov-Session` trailer of the branch's own commits names.

What it deletes:

- The worktree of the branch, with `git worktree remove --force`, and the worktree record. Changes that are not committed are lost.
- The branch, with `git branch -D`.
- The record of each session of the discard.
- Each archive `<logs_dir>/<agent>/<session>-*.tar.gz` of each session of the discard.

What it keeps:

- The transcripts of the agent itself, for example in `~/.claude/projects`. They belong to the agent.
- git-ai notes. `prov` never writes git-ai data. A note on a commit that no ref reaches any more has no effect.
- A remote branch. `prov discard` names it and prints the command that deletes it.

It refuses in each of these cases:

- It runs in an agent worktree, or an agent environment variable is set. Only a human runs it.
- The branch is checked out in the worktree where it runs.
- A session of the discard is Open, also when it is stale, or Closing. Archive it with `prov session archive` first. A Failed session does not stop it.
- A `Prov-Session` trailer in a commit on another ref names a session of the discard. Reason: an integration group, or an agent commit that is already merged or is on another agent branch, needs the record or the archive of that session. To delete only the worktree, use `prov finish`.
- A session that a trailer of the branch's own commits names has a record for another branch. Discard that branch first, or drop the record.
- A `Copied-From` trailer in a commit on another ref, or a copy record, names one of the branch's own commits as its source. Reason: "Copying files" says that the transcript of an inherited session is found through `Copied-From`.

Output and options:

- `prov discard` prints everything that it will delete: the worktree with the number of changed files, the branch with the number of its own commits, each session and each archive file. It deletes nothing unless `--yes` is given.
- `--dry-run` prints the same list and never deletes.
- `--json` gives the same list.

**[R-DISC-1]** Order of the steps:

1. Without the lock: find the branch, its own commits and the sessions of the discard, and run the checks on the trailers and the copy records. Save the value of every ref. Reason: this reads the history, and a hook must not wait for that.
2. Under the lock: check the states of the sessions again. Refuse if a merge is in progress in any worktree of the repository whose `MERGE_HEAD` reaches one of the branch's own commits. If a ref changed since step 1, release the lock and start again from step 1, at most three times; then refuse. Refs under `refs/notes/` are not compared, because git-ai changes them at every commit. Then delete the archives and the session records, and release the lock.
3. Remove the worktree and delete the worktree record.
4. Delete the branch.

- The merge gate reads the session records under the lock, and holds it until it has written the trailers (owner decision, 2026-11-13). Reason: git writes `MERGE_HEAD` before the gate runs. A gate that runs before step 2 therefore makes step 2 refuse, and a gate that runs after step 2 finds no record and refuses. The model `ProvDiscard.bug-gate-unlocked` shows the failure without the lock: discard deletes the archive between the gate and the commit, and the commit names an archive that no longer exists.
- Reason for the ref check: a merge commit can land between step 1 and step 2, and its trailers then name a session of the discard (`ProvDiscard.bug-no-ref-recheck`). Reason for the merge check: a merge whose gate has passed is not on a ref yet (`ProvDiscard.bug-no-merge-check`).
- If `prov discard` stops after step 2, running it again by branch name finishes steps 3 and 4. The records are gone then, so it finds no sessions, and every archive was deleted in step 2.
- Accepted limit: if `prov` does not know the process ID of an agent (R-SESS-3), its next event after the discard makes a new record for a branch that no longer exists. `prov doctor` reports that record as stale, and `prov session drop` removes it.

`prov discard` is the only command that deletes a transcript archive or a branch (owner decision, 2026-11-13, with the name and the refusal when another ref names a session of the discard).

## Commit identity

Agent commits use the author name and email of the agent. Each adapter has a default identity, and the config can replace it. Agent commits are not signed: agent commands get `commit.gpgsign=false` through `GIT_CONFIG_COUNT`. Reason: an agent must not sign with a human's key.

- Claude Code: `prov init` writes the identity and the signing setting as `env` in `.claude/settings.json`. Every command that Claude Code runs gets them. Use `Claude <noreply@anthropic.com>`. Reason: git-ai recognizes this identity. When a committed line has no checkpoint, git-ai then attributes the line to Claude. With any other identity, git-ai attributes the line to the committer as a known human.
- In `pre-commit`, `prov` identifies a commit by an agent through the author email from the config, or through an agent environment variable (`CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`).
- In an integration, the merge gate does not use the author email. Every commit in the incoming range is an agent commit. Reason: the agent sets its own author email.
- The test for a human commit uses the author email. This test trusts the agent's identity: an agent that commits with a human's email passes as a human. `prov` does not check signatures. For a stronger control, use the forge's branch protection to require signed commits on the human branches.

## Hooks

### Agent hooks

| Agent | Events | Installed in |
|---|---|---|
| Claude Code | SessionStart, UserPromptSubmit, Stop, SessionEnd | `hooks` in `.claude/settings.json` |

Each hook runs `prov hook <adapter> <event>` with the event JSON on stdin. `prov init` merges the Claude Code entries into the settings file and keeps all other keys. The SessionEnd hook only marks the session Closing and starts a detached archive process. Reason: the agent gives a SessionEnd hook little time.

### Git hooks

`prov init` writes one-line shims to the hooks directory. Each shim runs `exec prov hook git <name> "$@"`. All worktrees use the same shims.

The hooks directory must be outside every worktree. **Reason**: during a merge, git runs hooks after it writes the merged files. A shim in a checkout would then be the agent branch's copy, and a branch could switch off the merge gate.

- Without `core.hooksPath`, the hooks directory is `.git/hooks/` in the common git directory. This is the default.
- `prov init` accepts a `core.hooksPath` that is an absolute path outside every worktree. It stops if the path is relative or inside a worktree, for example `.githooks`.
- If a hook with the same name exists in the hooks directory, `prov init` keeps it as `<name>.local`. `prov hook` runs it after its own work, with the same arguments. If it fails, the hook fails.
- The shims are not tracked by git. In a new clone, run `prov init` again. It installs the shims and the git settings and leaves tracked files unchanged.

| Hook | Jobs |
|---|---|
| `pre-commit` | Refuse a commit by an agent on a branch that does not match `agent/**`. Do the service check for every commit. In an agent worktree: refuse if the worktree is contended; do the sweep if the worktree has one occupant. Do the copy guard for each copied file. |
| `prepare-commit-msg` | For an integration: run the merge gate, then add the integration trailers. For a commit in an agent worktree with one occupant: add `Prov-Session` for the occupant. For all commits: add the `Prov-Approved` trailers of `--human` and `--ai` approvals, and the `Copied-From` trailers. |
| `post-commit`, `post-merge` | After an integration: delete the session records that R-INT-2 permits, and print the hint for `prov revise`. After a commit with copied files: start the deferred copy check. After a fast-forward of a human branch: see "Squash, fast-forward, rebase and cherry-pick". |

**[R-SWEEP-1]** In an agent worktree with one occupant, the sweep makes a checkpoint of the changes for that occupant, except tainted files (R-SWEEP-2).

In an agent worktree with no occupant, `pre-commit` runs no sweep. The changes there are tainted (R-SWEEP-2), so they become doubtful lines and the human decides at the gate. Reason: `prov` must not attribute a change to a session that did not make it.

There is no `pre-merge-commit` shim. Reason: `MERGE_HEAD` does not exist when that hook runs, and `--no-verify` skips it. `prepare-commit-msg` has `MERGE_HEAD` and runs also with `--no-verify`.

### Limits of local hooks

Hooks are advisory for an agent. An agent can commit with `--no-verify`, write to the hooks directory, or change `core.hooksPath`. The controls are these:

- the merge gate, which runs from the hooks directory during integration
- `prov check`, which does the same check on a human branch without local hooks
- `prov doctor`, which checks the hooks directory and the shim contents

### No pre-commit gate

`prov` does not check line authorship before an ordinary commit. Only integrations are gated. Reason: git-ai's counts before a commit are not reliable. In a test, `git-ai status --json --diff-only` counted 0 lines for a new file, and it counted `known_human` lines as unknown.

## Keeping attribution complete

git-ai attributes a line through the checkpoints around the change. git-ai applies its recovery steps to a committed line with no checkpoint. What a committed line with no checkpoint becomes depends on the commit (tested with git-ai 1.7.6):

| Commit author | Other lines of the commit | A line with no checkpoint becomes |
|---|---|---|
| An agent identity | lines of an AI session | a line of that session |
| An agent identity | none of an AI session | a line of a session that git-ai invents |
| A human | lines of an AI session | untracked |
| A human | none of an AI session | `known_human`, for the committer |

So in an agent commit, git-ai gives every line with no checkpoint to an AI session, and the gate can see it only when that session is invented. `prov` prevents the four causes:

| Cause | Control |
|---|---|
| The agent commit identity is not recognized by git-ai | Use the agent identity in "Commit identity". |
| A process that the agent started writes after the tool call ends | Sweep. In an agent worktree with one occupant, every change that is not tainted is the occupant's change. `prov` runs `git-ai checkpoint agent-v1` with type `ai_agent` on the changed files. The adapter decides when the sweep runs. For Claude Code, it runs at each Stop, at SessionEnd and in `pre-commit`. At the end of `pre-commit`, `prov` runs `git-ai await`, because a checkpoint is applied after the command returns, and a commit right after it can be recorded first (see the evidence). **[R-SWEEP-5]** The sweep at SessionEnd runs for the session that ends, before its state changes, if that session is the only occupant. Reason: a session can end with no Stop, for example after Ctrl-C during a turn. Its changes would then stay without a checkpoint, and they would be tainted when the next session starts. The agent name is `claude`, which is also git-ai's name for the tool. The conversation ID is the session ID. The model comes from "Agent model". |
| The git-ai service is not available | `git-ai bg status --repo <worktree>` gives exit code 1 when the service is not available. A checkpoint then exits 0 and records nothing. At SessionStart, `prov` shows a warning and runs `git-ai bg start`. `pre-commit` refuses the commit, on every branch. |
| The git-ai integration of an agent is not installed | `prov doctor` checks the integration of each configured agent. |

- An agent must commit where the git-ai service is reachable. It must not commit inside a sandbox that blocks the service. Reason: `pre-commit` refuses the commit when the service check fails.
- `pre-commit` refuses a commit on a human branch too when the service is not available. There is no option that changes the refusal to a warning. `git commit --no-verify` skips it, as it skips every hook. Reason: git-ai moves a note to the new commit of an amend only if its service sees the amend. While the service is not available, a human commit gets no note, and the note of an amended commit stays on the old commit. Nothing reports either case later. This happened in the `prov` repository: an amend of the root commit while the service was not running left the new commit with no note, and git-ai did not repair it after the service started again.
    - `git rebase` and `git cherry-pick` do not run `pre-commit` for the commits that they make, and `git merge` does not run it for the merge commit. The check does not cover them. This is an accepted limit.
- The human does not edit files in an agent worktree.
- **[R-SWEEP-4]** Processes that continue after SessionEnd, for example a development server, are not swept, and the sweep of a later session would take their changes. Stop them before another session starts in the worktree, and before `prov finish`. This is an accepted limit. The model `ProvSweep.limit-background` records it.
    - `prov session archive`, `prov finish` and `prov start --resume` (in a worktree that exists) list the processes whose current directory is in the worktree, as a warning (owner decision, 2026-11-12). Left out: `prov` itself, its ancestors, shells (a person's terminal writes no files by itself), and processes that are not there in two scans 300 ms apart (git and git-ai start short processes in a worktree). On Linux the scan reads `/proc`; on macOS it runs `lsof`. A process that writes into the worktree from another current directory is not found.

**[R-SWEEP-2] Taint.** Live state keeps, for each agent worktree, the *owner*: the session that `prov` last saw as the only occupant.

- Every `prov` hook that runs in an agent worktree (agent events and `pre-commit`) first calculates the occupants, under the lock. If the occupants are not exactly the owner, the changes that `git status` shows at that moment are tainted, and the owner becomes the single occupant, or none.
- SessionEnd calculates the occupants twice: before the session leaves, for its own sweep, and again after it is Closing. If one session is left, it becomes the owner at that moment (owner decision, 2026-11-14). Reason: otherwise the owner stays none after a contention ends, and the next event of the remaining session taints its own later work. A live run showed this (see "Appendix: test evidence").
- To taint the changes, `prov` runs an `agent-v1` checkpoint of type `ai_agent` on the changed files, with the tool `prov-unattributed`, the model `unknown` and a new conversation ID for each taint. No record and no `Prov-Session` trailer names such a session, so its lines are doubtful lines ("Sessions of an integration"), and the human decides at the gate. After the approval, the note still says that an AI wrote the lines, in a session that nobody knows. Reason: in an agent worktree, the changes almost always come from an agent or a process that it started. A `known_human` checkpoint would make the lines doubtful too, but after the approval the note would say that a human wrote them.
- A checkpoint covers only the changes since the checkpoint before it. Lines that git-ai recorded through the agent's own hooks, or through an earlier sweep, keep their attribution. Changes that the new occupant makes later are swept as usual.
- git-ai applies a checkpoint after the command returns. Before a taint, `prov` therefore runs `git-ai await` with a limit of 5 seconds, so that a checkpoint of the agent's own hooks for an edit just before is applied first (owner decision, 2026-11-14). If the wait fails, the taint still runs. Reason: in a live run, a taint at Stop took the lines of a file that the agent had written just before, through its own git-ai hook. The wait runs only when a taint runs, which is rare.
- If the checkpoint fails, the owner stays, and the next hook tries again. `pre-commit` then refuses the commit.

Reason: `prov` cannot see who made a change, only that a file changed. When the occupant changes, the changes before that can belong to the previous session, to a session that crashed, or to one of two sessions in a contended worktree. The safe result is a doubtful line, not a guess. Without this rule, the sweep gives the changes of a session that crashed to the next session.

It is not enough to leave such changes with no checkpoint. In an agent commit that also has lines of an AI session, git-ai gives a line with no checkpoint to that session (see the table in "Keeping attribution complete").

## Integration into a human branch

There is one way to integrate: a merge commit, made with `git merge <agent branch>`. Reason: the agent commits then stay in the history of the human branch, and `prov check` can check everything that the gate checked.

`prov init` sets two git options in the repository:

- `merge.ff = false`. A plain `git merge` then always makes a merge commit, so the gate always runs. This option applies to every branch of the repository, because every branch that is not an agent branch is a human branch. A merge of one human branch into another therefore also makes a merge commit.
- `pull.ff = only`. Reason: with `merge.ff = false` alone, a plain `git pull` on a human branch makes a merge commit also when the branch is only behind. With `pull.ff = only`, a pull fast-forwards or stops. This option applies to every branch of the repository.

### What counts as an integration

`prepare-commit-msg` decides this from the incoming commits, not from a branch name. A merge of a commit ID or of a remote branch is therefore treated the same as a merge of a local branch.

- The incoming tip is `MERGE_HEAD`.
- The incoming range is `HEAD..<incoming tip>`.
- The commit is an integration if one commit on the first-parent line of the incoming range is not a human commit.
- In an integration, every commit in the incoming range is an agent commit.
- A merge whose first-parent line holds only human commits is not an integration. Examples: a merge of a feature branch into `main`, and a merge of the same branch from another clone. The gate does not run. The integrations on the merged branch passed the gate when they were made, and `prov check <base>..<tip>` checks them again.
- An octopus merge that contains an integration is refused.

### Sessions of an integration

The sessions of an integration are those named in a `Prov-Session` trailer or in the note of an agent commit.

One exception: a session that only a note names, and that has no record. git-ai invents such a session for lines with no checkpoint in a commit with an agent identity, for example after `git commit --no-verify`, which skips the sweep. No transcript exists for it. It is not a session of the integration, and its lines are doubtful lines. `prov check <base>..<tip>` has no records: there, a session that only a note names is a session of the integration if the merge has a trailer group for it.

- The integration fails if one of these sessions is not Archived, or has no record. Archive an Open session with `prov session archive` first.
- The gate reads the records under the lock, and holds it until it has written the trailers (R-DISC-1).
- The integration fails while a session is Open in a worktree whose branch tip is in the incoming range.
- **[R-INT-1]** After the integration, `post-commit` reads the trailers of the commit that was made (`HEAD`), not the trailers that the gate added. Reason: git opens the editor after the gate, so the commit's trailers can differ from the gate's.
- **[R-INT-2]** It deletes the record of a session of the integration only if the record is Archived and the commit has a trailer group for that session with the record's transcript hash. It deletes no other record, also not the record of a session on the same branch that made no commit. Transcript archives stay. `prov doctor` shows a record that stays when its branch is merged into the branch that is checked out, and `prov session drop` removes it. Reason: a session can be resumed between the gate and `post-commit`, and its record is then in use; and a rule for other sessions would need to know whether their work is merged.
- Accepted limit: git opens the editor after the gate, and the human can change the message there, for example delete a trailer group. A session can also be resumed before the commit. No hook checks the message after the editor. `prov check <base>..<tip>` then reports the commit, and R-INT-2 keeps the records, so the human can make the integration again. The model `ProvIntegration.limit-incomplete-commit` records it. (A `commit-msg` hook, R-INT-3, checked this for a time on 2026-11-12. It was removed to keep one gate run per merge.)

### Squash, fast-forward, rebase and cherry-pick

Squash, fast-forward, rebase and cherry-pick of agent commits onto a human branch are not permitted. No hook that `prov` uses can refuse a fast-forward.

- `prepare-commit-msg` refuses the commit after `git merge --squash` if the squashed commits are not all human commits. To stop the squash, run `git reset --hard`.
- `post-merge` runs after a fast-forward. If a human branch moved onto commits that are not human commits, `prov` prints a warning and the command to undo it: `git reset --keep ORIG_HEAD`.
- `prov check` refuses a first-parent commit on a human branch that is not a human commit.

### Integration trailers

`prepare-commit-msg` adds trailers through `git interpret-trailers --if-exists add`. Each session of the integration gets one group, in start order. Each group starts with `Prov-Session`.

```text
Merge branch 'agent/claude/20261111-parser-tests'

Prov-Session: claude:5c1e2b9a-...
Prov-Transcript-SHA256: 9f2c...
Prov-Model: claude-fable-5-2
Prov-Session: claude:a71d04e3-...
Prov-Transcript-SHA256: 41b8...
Prov-Approved: notes.txt ack
Prov-Version: 0.1.0+<build SHA>
```

Each agent commit already has its own `Prov-Session` trailer. This records which session made which commit, and the subjects of those commits are the summary of the session. Which session made which line is in the git-ai notes. `prov` asks the agent for no other summary.

`prov` has no command to list integrated sessions. Use git:

```text
git log --first-parent --format='%h %s%n%(trailers:key=Prov-Session)' <human branch>
```

### Trailer format

Each trailer value is one line. `prov` refuses a value with a line break.

| Trailer | On | Value | Necessary |
|---|---|---|---|
| `Prov-Session` | each agent commit; each group of an integration | `<agent>:<session ID>`. `<agent>` is the agent's name, for example `claude`. | yes |
| `Prov-Transcript-SHA256` | each group | 64 lowercase hex characters | yes |
| `Prov-Approved` | the commit that the approval is for | `<path> <choice>`. The choice is the last word: `ack`, `human` or `ai`. The path can contain spaces. | one for each approval |
| `Prov-Model` | a group of an integration, after `Prov-Transcript-SHA256` | the model of the session from its record, the latest one if it changed. Reason: the git-ai note has the model `unknown` when every edit went through the agent's own git-ai hooks ("Agent model"). | if the record has a model |
| `Prov-Version` | each integration | `<version>+<build SHA>` | yes |
| `Prov-Revises` | a commit made by `prov revise` | the full commit ID of an integration that it revises | one for each revised integration |
| `Copied-From` | a commit with copied files | `<source>@<commit>:<path>`, followed by ` => <dest>` if the path in this repository is not `<path>`. `<source>` is the remote URL of the source repository, without a user name or password, or its absolute path if it has no remote. | one for each copied file |

## The merge gate

No doubtful line reaches a human branch without a human approval. `prov` reads each agent commit with `git-ai diff <commit> --json`. An added line is doubtful when one of these is true:

- No range in the output gives it a session or a human (untracked).
- A range gives it a human. Reason: in an agent commit, this label comes from git-ai's fallback, not from a person.
- Its agent commit has no note.
- A range gives it a session that no `Prov-Session` trailer names and that has no record. See "Sessions of an integration".

Two kinds of file are doubtful as a whole, with one item for the file:

- The agent commit adds lines to the file, changes it as a binary file, or changes its mode or its type, and the output of git-ai has no added range for that file. Reason: git-ai ignores some paths, for example `**/vendor/**`, lock files and `*.min.js`. It drops the checkpoints for such a file and lists none of its lines. It also lists no range for a binary file or a change of mode. So an agent's change there would otherwise never be doubtful.
- The agent commit deletes the file. Reason: git-ai records no author for a deletion, and a file that disappears is easy to miss in a review.

Lines that an agent commit deletes from a file that stays are not doubtful. Reason: a deleted line has no author to doubt.

One `git diff-tree --raw --numstat` for each agent commit gives these changes. A symbolic link and a submodule need no rule of their own: git-ai gives a symbolic link a range, and a submodule an untracked range.

A merge commit in the incoming range is not read with `git-ai diff`. Reason: git-ai shows a merge as the difference from its first parent, which repeats the lines of the merged commits. Instead, each path that the merge commit changed relative to every parent is doubtful. This is what the author of the merge wrote, for example an agent that merged the human branch into its branch and resolved a conflict. A clean merge has no such path.

A change to a protected path is also doubtful. The default protected paths are `.provenance.toml`, `.claude/**`, `.github/workflows/**`, `CLAUDE.md` and `AGENTS.md`. Reason: an agent could change these files on its own branch to weaken the controls. The instruction files `CLAUDE.md` and `AGENTS.md` have no `/`, so they match at every level: Claude Code also reads them in subdirectories (owner decision, 2026-11-13).

The gate runs in `prepare-commit-msg` for each integration. It refuses at once when an agent environment variable is set (see "Only a human can integrate" under `prov approve`). Then:

1. It reads the config with `git show HEAD:.provenance.toml`. If the binary version does not satisfy `required_version`, the integration fails.
2. It runs `git-ai await --timeout 60`. If the await fails, the gate shows a warning and continues. For each agent commit of the last two minutes that has no note yet, it waits for the note. All these waits together last up to 60 seconds. A commit that still has no note gives items that need an approval. Reason: in a test under load on a slow file system, `git-ai await` gave up after 6 seconds although every note was written.
3. It checks the sessions of the integration. See "Sessions of an integration".
4. It lists each doubtful range, each path that a merge commit in the incoming range changed, and each changed protected path. If the merge message has no `Prov-Approved` trailer for the path of an item, the commit fails.

The gate reads the config from `HEAD`. Reason: during a merge, the file in the worktree is already the merged file, with the agent's changes. `HEAD` is the human branch before the integration.

### When the gate refuses

git leaves the merge in progress. `MERGE_HEAD` exists and the changes are staged. The gate prints each open item and the next steps:

1. Run `prov approve --merge <path>...` for the open items.
2. Run `git commit`. The gate runs again.

To stop instead, run `git merge --abort`.

### Approval

`prov approve <path>...` records a human decision for each path. Give exactly one of `--merge`, `--human` and `--ai`.

| Option | When | Effect |
|---|---|---|
| `--merge` | While a merge is in progress, after the gate refused | The lines keep their git-ai attribution. `prov` adds `Prov-Approved: <path> ack` to git's merge message (`.git/MERGE_MSG`). |
| `--human` | Before your own commit | Runs `git-ai checkpoint known_human --editor prov --editor-version <version>`. Recorded as `human`. |
| `--ai [--agent <tool>] [--model <model>]` | Before your own commit | Runs `git-ai checkpoint agent-v1` with type `ai_agent`. Recorded as `ai`. The default for `--agent` and `--model` is `unknown`. |

- Only a human can approve. `prov approve` refuses in every agent worktree, and when an agent environment variable is set.
- Only a human can integrate. The gate refuses an integration when an agent environment variable is set. Reason: `git merge` runs no `pre-commit`, so the refusal of an agent commit on a human branch does not cover a merge commit. An agent session in a human worktree could otherwise merge an agent branch and write `Prov-Approved` trailers into the merge message itself.
- A `Prov-Approved` trailer records the decision of the human who made the integration. It does not show that `prov approve` was used: a human can also write the trailer by hand.
- `prov approve` asks for a typed `yes` at a terminal. It reads the answer from `/dev/tty`. With no terminal, it refuses; `--dry-run` shows the items without a question. Reason: an approval is a person's decision, so a script or an agent cannot pipe in the answer (owner decision, 2026-11-13). For a protected path, it first shows the diff of that path between the human branch (`HEAD` of the merge) and the incoming tip.
- A merge approval exists only in the merge message of the merge in progress. `git merge --abort` discards it. `prov` keeps no other record of it, and you cannot approve before you start the merge.
- `--human` and `--ai` are for lines whose author git-ai cannot know. Examples: code that you paste from an AI tool in a browser (`--ai`), and the output of a generator that you ran (`--human`). The approval stays in live state with the git blob ID of the file content. Reason: git computes it already, and it is the ID that git uses for the content (owner decision, 2026-11-13). A change to the file cancels it.
- `--human` and `--ai` apply to a complete file: to every change in it since the last checkpoint. They cannot mark a part of the changes.
- At the commit, `prepare-commit-msg` adds `Prov-Approved: <path> human` or `Prov-Approved: <path> ai`. Then `prov` deletes the live state.
- A checkpoint affects only the next commit. To change the authorship of committed lines, see "Replacing the authorship of a file".

## Checking: `prov check`

```text
prov check [<agent branch> | <base>..<tip>]
```

`prov check` applies the rules of the merge gate without a merge. It changes nothing. Exit code 1 if one check fails.

| Form | Use | What it does |
|---|---|---|
| `prov check` | The human, to see what waits | Lists each agent branch that is not integrated into the target, with its commits, its sessions and the number of doubtful lines. |
| `prov check <agent branch>` | The human, before a merge | Does steps 2 to 4 of the gate for that branch. It lists the sessions that are not Archived, each doubtful range and each changed protected path. These are the items that the gate will show. |
| `prov check <base>..<tip>` | CI, on a human branch | Checks commits that are already on a human branch. It uses no local hooks and no live state, only what a clone holds. |

The first two forms compare against a target: the human branch that the merge will go into. The target is the branch that is checked out. `--into <branch>` names a different target. In an agent worktree or with a detached `HEAD`, `--into` is necessary.

The exit code of each form is 0 only if nothing fails. For `prov check <agent branch>`, exit code 0 means that a plain `git merge` of the branch passes the gate now: an item that still needs an approval gives exit code 1.

### Checks on a human branch

`prov check <base>..<tip>` first checks that `.provenance.toml` at `<tip>` parses and that the binary version satisfies `required_version`. Then, for each commit on the first-parent line of the range, and on the first-parent line of each human branch that a merge in the range brings in:

| Commit | Checks |
|---|---|
| Every commit | It is a human commit. |
| A merge that brings in agent commits | Each session named in a trailer or a note of an agent commit has a trailer group. `Prov-Version` is present. Each doubtful range and each changed protected path has a `Prov-Approved` trailer. The config for these checks is the file at the first parent. A merge without these trailers fails; this is how a forge merge appears. |
| Any other commit | Its note names no AI session, except for a path with a `Copied-From` trailer or a `Prov-Approved` trailer with the choice `ai`. This is how a squash or a cherry-pick of agent commits appears, and how the work of a session in a human worktree appears. |

The CI job must provide these:

- `prov` and git-ai, installed
- the full history, not a shallow clone
- `refs/notes/ai`, fetched

`prov check` does not need the git-ai service. `git-ai diff`, `git-ai blame` and `git-ai stats` read notes without it.

**Limit**: `prov check` cannot find a squash that has no note, for example a squash that a forge made. Such a commit looks like a human commit. The branch protection in "Non-goals" prevents it.

`prov check <base>..<tip>` warns about each commit on a checked first-parent line that is not a merge, adds lines and has no note. The warning does not fail the check. It skips commits older than the first commit of `refs/notes/ai` and the commits in `known_gaps`, as `prov doctor` does. If the clone has no `refs/notes/ai`, it warns once. Reason: a squash made on a forge has no note, and nothing else reports it. A commit by a person who does not use git-ai has no note either, so a failure would stop such a team (owner decision, 2026-11-13). The check reads the notes with one `git notes list` and the commit times with one `git log`.

**Rule**: a human branch goes into another human branch only through a merge commit, also on a forge, for example the pull request of a feature branch into `main`. Turn off squash merges and rebase merges for the protected branches on the forge. Reason: a merge commit keeps every commit with its note and the integrations with their trailers. A squash on the forge makes one commit with no note, so the AI lines in it look human, and `prov check` cannot find it (the limit above). A rebase on the forge drops the integrations and puts the agent commits on the first-parent line; `prov check` reports it, but the record is lost from that branch.

git-ai pushes `refs/notes/ai` with a plain `git push` while its service runs, and a clone fetches it. A CI job without the service must fetch the notes itself.

## Revising an integration: `prov revise`

```text
prov revise [--of <merge>]... [--dry-run]
```

A human cannot edit an agent branch. The place to correct agent work is a human branch, after the integration. `prov revise` commits the human's staged changes to the lines of an integration and records which integration they revise. It is optional. Reason: the merge is the human's approval, and `prov` does not copy a pull request (owner decision, 2026-11-13). `prov revise` adds a record that a human changed agent work. It adds no gate.

- `prov` has no rule about the branch for a revision. A team can revise on `main` or on a branch of its own.
- After an integration, `post-commit` prints one line. It names the merge and suggests `prov revise`. The hook runs no extra git process for it.
- A merge can have several revisions, and a revision can name several merges.

### What `prov revise` does

1. It refuses in every agent worktree, and when an agent environment variable is set. Reason: a revision is a human's change, like an approval.
2. It finds the targets. Each `--of <merge>` names one target. It refuses if the commit is not an integration or is not an ancestor of `HEAD`. Without `--of`, the one target is the first commit on the first-parent line from `HEAD` that is a merge with a `Prov-Session` trailer. It refuses if there is none. Reason for several targets: a fix can cross the work of two integrations, for example an interface between two modules that two sessions ported (owner decision, 2026-11-13). `prov` does not find several targets by itself. Reason: it would have to find the merge that brought in each blamed commit. For each target, it prints the number of commits between the target and `HEAD`, as information.
3. It refuses if nothing is staged, and if a file with staged changes also has changes that are not staged. Reason: git-ai records the file in the working tree, not the staged content.
4. **[R-REV-1]** It reads the staged diff against `HEAD` with `-U0`.
   - A line of `HEAD` counts when the diff changes or deletes it.
   - Lines that the diff only inserts (a hunk `@@ -N,0 +M,k @@`) count when line `N` or line `N+1` of `HEAD` is a merged line of a target. `N` is the line after which they go, and 0 at the start of the file. Reason: in a port, a common correction is an insertion, for example a missing branch or error check (owner decision, 2026-11-13). The lines of a new file never count.
   - A line of `HEAD` is a merged line of a target when its commit is the target, a commit in `<target>^1..<target>^2`, or an earlier revision of the target: a commit with a `Prov-Revises: <target>` trailer. Reason: correction is iterative, and a second pass over revised lines revises the same work (owner decision, 2026-11-13). Each revision names the target, not the revision before it, so this holds for any number of passes.
   - For each file with counted lines, one `git blame HEAD -L <range> ... -- <path>` call gives the commit of each changed line and of each neighbour of the inserted lines. One `git log --no-walk` call reads the `Prov-Revises` trailers of the blamed commits that are not merged commits of a target.
   - The commit refuses if a target has no counted line that is a merged line of it, changed, deleted or inserted next to. Reason: a revision must change the work that it names (owner decision, 2026-11-13). This also refuses an empty revision, and a revision that only adds a new file, for example a new test.
5. For each target, it prints the number of its merged lines that the revision changes or deletes, and the number of lines that it inserts next to them. Then it prints the number of other changed lines. Other changes in the same commit are permitted. With `--dry-run`, it stops here.
6. It starts `git commit --template <file>`, and git opens the editor. The template has an empty subject line, one trailer `Prov-Revises: <target>` for each target, and comment lines with each target's short ID and subject and the counts of step 5. The human writes the message. git refuses the commit if the message is not changed. With `core.commentChar = auto`, the template has no comment lines. Reason: git would then choose a comment character that no template line starts with, and keep those lines. Reason: the human describes the revision; `prov` adds no words to it. The git hooks run as for every commit. `prov revise` makes no git-ai checkpoint. Reason: git-ai gives the lines of a human commit with no checkpoint to the committer as `known_human`. A `known_human` checkpoint gives the same note.

Accepted limit: the human can delete the trailer in the editor. The commit is then an ordinary human commit.

Consequences of R-REV-1:

- A second revision of the same merge counts the lines that an earlier revision of it changed. A line that a plain human commit changed is not a merged line.
- An insertion between two lines that are not merged lines does not count, also inside a file of the merge.
- A line that a conflict resolution wrote is blamed to the merge. It is a merged line. A line that the resolution kept from the human branch is not.
- A binary file has no lines in the diff. A change to it is never a merged line.
- Accepted limit: an agent branch can merge a human branch that is not in the target branch, for example a feature branch. The commits of that branch are then in `<target>^1..<target>^2`, and their lines count as merged lines. This agrees with "What counts as an integration": every commit in the incoming range of an integration is an agent commit.

### A revision on a human branch

A revision commit is a human commit. `prov check` checks it as "any other commit" and needs no change for it. `prov check` does not read `Prov-Revises`. Reason: the trailer is a record, not a control. A person can write it by hand, and a revision is optional.

## Copying files: `prov cp`

```text
prov cp --from <repo> [--rev <rev>] <path>... <dest> [options]
```

`prov cp` copies files, as they are at a commit, into the current repository, with an author for every line. `<repo>` is the path of a local clone. `<path>` is relative to the root of the source. A `<path>` can be a directory: `prov cp` then copies each file under it that the source commit tracks. The default `<rev>` is `HEAD`.

- Only a human can run it. It refuses in every agent worktree, and when an agent environment variable is set.
- It refuses if a destination file exists. To replace a file, see "Replacing the authorship of a file".
- It preserves the executable bit.
- It copies a binary file or a symbolic link unchanged, with no line authorship, and shows it in the plan (`binary = true` or `symlink = true`).
- One source path that is a file: `<dest>` is the new path of the file, or a directory if it ends with `/` or exists as a directory. One source path that is a directory: `<dest>` is the new path of that directory. More than one source path: `<dest>` is a directory, and each source goes into it under its own name.

### The plan

`prov cp` works in two steps. First it writes a plan: each file, and for each file the line ranges with their authors. Then it applies the plan. Every line must have an author before the plan is applied. Reason: git-ai's recovery would attach a line without one to a nearby AI edit or to the person who copies.

The authors come from these sources, in this order:

1. `--all human` or `--all ai [--agent <tool>] [--model <model>] [--session <id>]` sets the author of every line and ignores the source's notes. Use it when the source has no git-ai records, or when its records are wrong.
2. Without `--all`, each range that `git-ai blame --json` attributes to an AI session in the source keeps that session: its tool, model and conversation ID. The git-ai session key is then the same as in the source.
3. `--missing human` or `--missing ai [--agent <tool>] [--model <model>]` sets the author of every range that still has none.
4. A range that still has no author is unresolved.

- The default for `--agent` and `--model` is `unknown`.
- Without `--session`, the conversation ID of an `ai` range is the plan ID.
- A `human` range is attributed to the person who copies, under the author that git-ai is configured with. This is also true for a line that a different human wrote in the source. Only AI sessions are inherited from the source.
- If the source has no `refs/notes/ai`, `prov cp` says so. Every range is then unresolved unless `--all` or `--missing` is given.

Plan format:

```toml
[source]
repo = "/home/me/other-repo"
commit = "4f2a9c1e..."

[[file]]
path = "src/parse.py"
dest = "src/parse.py"
ranges = [
  { lines = "1-4",   author = "human" },
  { lines = "5-9",   author = "ai", agent = "claude", model = "m1", session = "s1" },
  { lines = "10-12", author = "?" },
]

[[file]]
path = "img/logo.png"
dest = "img/logo.png"
binary = true
```

`author = "?"` marks an unresolved range.

Resolving the plan:

- If no range is unresolved, `prov cp` prints the plan and applies it.
- If a range is unresolved and the command runs at a terminal, `prov cp` opens the plan in the editor from `GIT_EDITOR`, `core.editor`, `VISUAL` or `EDITOR`, as `git commit` does. You set authors, and you can split a file into more ranges. `prov cp` checks the plan when the editor closes.
- If a range is still unresolved, or the command does not run at a terminal, `prov cp` names the ranges and stops. It writes nothing to the working tree.
- `--plan <file>` writes the plan to a file and stops. `--apply <file>` applies a plan file. Use these for scripts, and when an agent prepares a plan for the human.
- The printed summary has one line for each file: path, line count and authors. It shows the ranges only for a file with more than one author.

Rules for a plan:

- The ranges of a file cover every line once.
- git-ai keeps one model for each session. A plan that gives one session two models fails.
- If identical lines of a file get different authors, `prov cp` shows a note. git-ai's diff cannot tell which copy each stage added, so the authors can move between identical lines. The content and the counts stay correct.

### Procedure

1. Read each file from the source commit, never from a working tree. `git-ai blame` reads a working tree and has no option for a commit. `prov cp` therefore runs it in a temporary clone of the source (`git clone --shared --no-checkout`, then a fetch of `refs/notes/ai` and a checkout of the commit) and deletes the clone. Nothing in the source repository changes.
2. Check that git-ai allows the repository and that no destination file exists.
3. Write and resolve the plan. Store it in `.git/provenance/copies/<id>.json`.
4. Replay each file. Write the file in stages, with one checkpoint after each stage:
   1. The human lines, then `git-ai checkpoint known_human --editor prov --editor-version <version>`.
   2. For each AI session: an `agent-v1` checkpoint of type `human`, that session's lines, then an `agent-v1` checkpoint of type `ai_agent`.

   The last stage makes the file equal to the source, byte for byte.
5. Stage the files. The human commits them. The commit can hold several copies and other changes. `pre-commit` refuses the commit if a copied file is different from the copied bytes. `prepare-commit-msg` adds one `Copied-From` trailer for each copied file. This copy guard applies to a file from the copy until a commit holds the file. A new copy to the same destination replaces an earlier copy that no commit holds.
6. Deferred check. `post-commit` starts a detached process. The process waits for the note for a maximum of 60 seconds. Then it compares the authors of each copied file with the plan, by content. In each group of identical lines, the set of authors must agree with the plan.
   - Agreement for every file: `prov` deletes the plan.
   - Authors moved between identical lines: `prov doctor` shows a note.
   - git-ai lists no line of the file, because it ignores the path (see "The merge gate"): the file has the result `ignored`, with no line authorship. `prov doctor` shows a note. The `Copied-From` trailer still records the origin.
   - Disagreement, or no note after 60 seconds: the plan stays, and `prov doctor` reports a failure for that file. A later copy of the same path that agrees clears the failure.

Session records and transcripts stay in the source repository. The copy has the line authorship and the `Copied-From` trailer. To find the transcript of an inherited session, follow `Copied-From` to the source.

### Replacing the authorship of a file

`prov doctor` finds commits with no note (see "Doctor checks"). For a note that an amend or a rebase left on the old commit, first see whether the rewrite can be made again while the git-ai service runs. That keeps every note as git-ai wrote it, and needs no copy.

No public git-ai command changes an existing note, and git-ai attributes only the lines that a commit changes. To correct the authorship of a committed file, remove it and copy it again:

1. `git rm <path>`, and commit.
2. `prov cp --from . --rev <commit before the removal> <path> <path>` with `--all`, or with a plan that you edit. Commit.

**Cost**: one commit of the history does not have the file, and `git blame` then shows the second commit for every line. Do this only when an authorship is wrong or a copy check failed.

## Trace export (optional)

`prov trace export [<range>]` writes Agent Trace records as JSON Lines, one record per commit with a note. The default range is the branch that is checked out. It finds the commits with a note with one `git notes --ref=ai list`, and reads each commit with `git-ai diff <commit> --json`. Reason: listing which commits have a note reads git, not the note format; asking git-ai for every commit would cost one process per commit (owner decision, 2026-11-13).

- The schema is Agent Trace 0.1.0, pinned in `vendor/agent-trace/0.1.0/` with its CC BY 4.0 notice. `prov` supports only this version.
- `prov` validates each record against the pinned schema before it writes the record. The validator is written in-house (`crates/prov/src/schema.rs`) and knows only the keywords that the pinned schema uses. A keyword that it does not know is an error, so a new schema cannot pass without notice. Reason: a full JSON Schema crate brings many dependencies for one small, fixed schema (owner decision, 2026-11-13).
- `version` is `"0.1"`. Reason: the schema requires the pattern `^[0-9]+\.[0-9]+$`.
- `id` is a version 5 UUID of the commit SHA, in a fixed namespace that the `prov` source defines.
- `timestamp` is the committer date of the commit.
- `vcs` is type `git` with the commit SHA as the revision.
- `tool` is name `prov` with the `prov` version.
- AI lines map to contributor type `ai` with `model_id`. Human lines map to `human`. Untracked lines map to `unknown`.
- `model_id` has the form `<provider>/<model>`. The adapter adds the provider when the note has only the model: `anthropic/` for Claude Code. A model that already has a provider stays unchanged. If the note has the model `unknown`, the model comes from the `Prov-Model` trailer of the session's integration, if one exists in the clone. If the model is still `unknown`, the record has no `model_id`.
- Two exports of the same range give identical bytes.

## Commands

| Command | Run by | Effect |
|---|---|---|
| `prov init [--agent <name>]... [--force]` | Human, once in each clone | Writes `.provenance.toml`, the shims in the hooks directory and the Claude Code settings. Sets `merge.ff` and `pull.ff`. Adds `logs_dir` to `.gitignore` if it is inside a worktree. Keeps an existing `.provenance.toml` that parses, and refuses to overwrite another changed file or hook without `--force`. `--force` always writes `.provenance.toml` again from the defaults, with the agents of `--agent` (owner decision, 2026-11-13). Reason: `--force` then has one meaning, a reset to the defaults. Commit your own settings first; git shows what the reset changed. |
| `prov doctor [--deep]` | Human | Checks the items in "Doctor checks". Exit code 1 if one check fails. |
| `prov check [<agent branch> \| <base>..<tip>]` | Human, CI | See "Checking". |
| `prov start <agent>` | Human | See "Starting a session". |
| `prov hook <adapter> <event>` | Shims only | Handles an agent or git event. |
| `prov session list` / `show <id>` / `close <id>` / `drop <id>` | Human | Shows sessions, closes one, or deletes the record of one. |
| `prov finish <session or branch> [--override]` | Human | See "Finishing". |
| `prov discard <session or branch> [--yes]` | Human | See "Discarding". |
| `prov completions [--shell <bash\|zsh\|fish>] [--install]` | Human | Prints the line that turns on tab completion (`source <(COMPLETE=bash prov)`). `--install` appends it to the shell's startup file (`~/.bashrc`, `$ZDOTDIR/.zshrc`, `$XDG_CONFIG_HOME/fish/config.fish`) unless a line with `COMPLETE=<shell> prov` is there. The default shell is `$SHELL`. `prov init` never changes the startup file, because it belongs to the user, not to the clone; it prints a hint when the line is missing (owner decision, 2026-11-12). Completion candidates come from the repository of the current directory: branches and session IDs. |
| `prov approve <path>...` | Human | See "Approval". |
| `prov revise [--of <merge>] [--dry-run]` | Human | See "Revising an integration". |
| `prov cp --from <repo> [--rev <rev>] <path>... <dest>` | Human | See "Copying files". |
| `prov trace export [<range>]` | Human | See "Trace export". |

### Doctor checks

`prov doctor` checks the local repository. It is not for CI; CI runs `prov check <base>..<tip>`.

- `.provenance.toml` parses, and the binary version satisfies `required_version`.
- The hooks directory is outside every worktree. Each shim has the expected content.
- `merge.ff` is `false`, and `pull.ff` is `only`.
- The agent environment sets `commit.gpgsign=false` and the agent identity, in `.claude/settings.json`.
- `.provenance.toml` and the agent settings files are committed.
- git-ai is installed, its version is the compatibility baseline or later (a version newer than the tested version is a warning, not a failure), its service is available, and it allows the repository. With `--deep`, `git-ai debug` passes. git-ai has no command that answers whether it allows a repository, so `prov doctor` reads `allow_repositories` and `exclude_repositories` with `git-ai config` and repeats git-ai's rule (see "Requirements"). A contract test checks the rule against git-ai (owner decision, 2026-11-13).
- The git-ai integration of each configured agent is installed.
- No session is stale or Failed. No worktree is contended. No copy check failed.
- A note for each record of a session that is not Open whose branch is merged into the branch that is checked out, with the `prov session drop` command (R-INT-2).
- `prov doctor` removes the record of an agent worktree that git no longer lists, and says so. This is the only change that it makes. Reason: the record holds only the owner of the worktree (R-SWEEP-2), and it stays when a worktree is removed with `git worktree remove` instead of `prov finish`.
- Every commit on a local branch, human or agent, that adds lines has a git-ai note. Merge commits and commits older than the first commit of `refs/notes/ai` are not checked. Before the check, `prov doctor` runs `git-ai await` if the service is available. Reason: git-ai writes a note for every commit that its service sees, so a commit with no note is one that git-ai did not see. Nothing else reports it.
    - For a commit with no note, `prov doctor` looks for a commit that no branch contains, with a note and the same `git patch-id --stable`. If it finds one, an amend or a rebase made the commit while git-ai did not see it. The report names the old commit: on a branch that no one else has, make the rewrite again from that commit while the service runs, and git-ai moves the note. On a public branch, replace the authorship of the files (see "Replacing the authorship of a file").
    - A commit in `known_gaps` in the config is listed as a note, not as a failure. An entry in `known_gaps` that is not a commit with no note is also listed as a note.

## Configuration

`.provenance.toml` is in the repository root and is tracked by git.

```toml
required_version = "=0.1.0"        # exact version. The merge gate and prov check refuse on a mismatch.
agents = ["claude"]                # the agents in use
logs_dir = "llm-logs"              # relative to the base directory
protected = [".provenance.toml", ".claude/**", ".github/workflows/**", "CLAUDE.md", "AGENTS.md"]
known_gaps = []                    # commits with no git-ai note that the human accepts. See "Doctor checks".

[agent.claude]                     # optional. Replaces a default of the adapter.
command = ["claude", "--model", "opus"]
```

Everything else is a fixed convention:

| Convention | Value |
|---|---|
| Agent branches | `agent/**` |
| Name of a new agent branch | `agent/<agent>/<YYYYMMDD>-<name>` |
| Worktrees | `../<repo>.worktrees/`, from the base directory |
| Agent names | `claude` (adapter `claude-code`) |
| Default identity and start command | From the adapter: `Claude <noreply@anthropic.com>` and `claude` |
| Stale session | 24 hours without an event |

- An `[agent.<name>]` section can replace `author`, `email` and `command`.
- `known_gaps` holds commit IDs, each with 7 or more hex characters. It is the record of a gap that the human accepts instead of a repair. Because `.provenance.toml` is a protected path, an agent branch cannot add to it without a human approval at the gate.
- Hooks read the config from their own checkout. The merge gate reads it from `HEAD`, and `prov check` reads it from the first parent of each integration.
- `required_version` is enforced by the merge gate and by `prov check`. On a mismatch, all other hooks show a warning and continue. Reason: an agent branch that started before a version change still has the old value in its checkout.

## Live state

All files are in `.git/provenance/` in the common git directory, so every worktree sees them. All writes use one advisory file lock, `lock`. Every read-modify-write of a record holds the lock from the read to the write.

| File | Content | Deleted when |
|---|---|---|
| `sessions/<id>.json` | state, generation, agent, worktree, branch, model, transcript path, archive file and hash, error text (Failed), agent PID, start time, last event time, `prov` version | an integration permits it (R-INT-2), `prov session drop`, or `prov discard` |
| `worktrees/<key>.json` | for one agent worktree: the owner session (R-SWEEP-2) | `prov finish` or `prov discard` removes the worktree, or `prov doctor` finds that git no longer lists it |
| `copies/<id>.json` | the plan, the blob ID of each copied file, the check result of each file | the copy check agrees for every file. A failed file is cleared when a later copy of the same path agrees. A record with a `moved` or `ignored` result stays until the human deletes the file: it is the only place that holds the planned authors of those lines. The note of `prov doctor` ends with the command that deletes it (owner decision, 2026-11-13). |
| `approvals/<key>.json` | path, choice (`human` or `ai`), content hash | the approval becomes a trailer. Merge approvals are not stored here. |

## Build and release

- No hosted CI or build service is used. The checks run on the developer's machine (see "Tests").
- Builds use `Cargo.lock` with `--locked`. The Rust toolchain is pinned in `rust-toolchain.toml`.
- `Prov-Version` names the version and the git SHA of the build.
- Releases: TBD. There is no release process yet; build from source. The binaries will be published on GitHub.
- The `prov` repository is developed with AI agents too. It uses `prov` on itself when version 0.1 works. Until then, its commits have git-ai attribution only.

| Crate | Contents |
|---|---|
| `prov-core` | config, live state, session state machine, lock, trailer format |
| `prov-git` | repository and worktree discovery, git commands, hook handling |
| `prov-adapters` | the adapter interface and the Claude Code adapter |
| `prov-gitai` | git-ai commands and their JSON output, copy plan and replay |
| `prov` | command line, init, doctor, check, detached processes |

## Tests

- Integration tests use temporary repositories with worktrees and recorded hook payloads. They compare live state, trailers and integration commits with expected output.
- A gate test integrates a branch that changes the config and the tracked agent settings. The gate must refuse it until the human approves.
- Attribution tests give each test its own home directory and its own git-ai service, and run every command with a clean environment. Reason: an agent session's own git-ai hooks attribute changes made by test commands to that session. A home of its own has no `allow_repositories`, so a test repository needs no remote.
- A copy test replays several files with different plans into one commit and checks each file against its plan.
- A property test checks that `prov` refuses a trailer value with a line break.
- A latency test fails if an agent hook (`prov hook <adapter> <event>`) takes more than 100 ms at the 95th percentile. On a slow file system, it uses the relative limit in "Agent hooks".
- `PROV_TEST_REPOS=<dir>` puts the test repositories on another file system. The suite must pass there too. The home directory of each test stays in `/tmp`, because git-ai needs a short local path for its sockets.
- A trace test exports one range twice and compares the bytes.
- Contract tests (`crates/prov/tests/gitai_contract.rs`) check, with git-ai alone, each premise about git-ai that a rule rests on: the JSON of `git-ai diff` and `git-ai blame`, the attribution of a file with no checkpoint in an agent commit and in a human commit, the taint checkpoint, the model rule, the sweep joining the agent's session, a checkpoint just before a commit, ignored paths, binary files, mode changes and deletions, `allow_repositories`, and blame of an empty file. They run before the tested version is raised.
- Local checks: `ci/run.sh [<commit> | --worktree]` runs the build, clippy with `-D warnings`, every test and the model tests in a clean Podman container. The image (`ci/Containerfile`) pins Ubuntu 26.04 with git 2.53, the Rust toolchain of `rust-toolchain.toml`, git-ai 1.7.6 and tla2tools 1.7.4, each download with its SHA-256. The tests run with `PROV_TEST_REQUIRE_GITAI=1`, so an integration test that cannot find git-ai fails instead of skipping. The pre-push hook `ci/pre-push` runs the checks for each commit that a push sends. Install it in each clone: `cp ci/pre-push "$(git rev-parse --git-path hooks)/pre-push"`.
- Model tests: the local checks run `tla/check-models.sh`. SANY parses every module. TLC must pass each `<Module>.cfg` and each config with another name. Each `*.bug-*`, `*.limit-*` and `*.reach-*` config must fail with an invariant violation (TLC exit code 12). Any other result fails the job.
- An archive test kills the archive process during the write, then runs the step again.
- The local checks run on Linux. On macOS, run `cargo test --workspace --locked` on a Mac before a release.

## Alternatives considered

| Option | For | Against | Result |
|---|---|---|---|
| Rust binary (this design) | No runtime in the target repository. Fast start for hooks that run at each prompt. Same language as git-ai. | Slower to change than a script. | Chosen |
| Python package installed with `uv tool install` | Fastest to build. | Needs Python and uv on every machine. About 100 ms start time for each hook. | Fallback if the Rust build stalls |
| Shell scripts copied into each repository | No build or release process. | Copied and edited for each project. Hard to test. Differences between macOS and Linux in every script. | Rejected |

Rust is chosen because hooks run at every prompt and at session end under a time limit, and because a binary with a pinned version is easy to cite.

## Open questions

None. The last one, where `prov` is published, was decided on 2026-11-12: a public GitHub repository, with binaries in GitHub releases. The crates are not published on crates.io (`publish = false`).

## Appendix: phase 2, OpenCode

OpenCode support is not in phase 1. This appendix keeps the design and the test results for phase 2. It is not a requirement for the first release.

Until phase 2, `prov` records no OpenCode session. Commits that OpenCode makes have no `Prov-Session` trailer and no transcript hash, so an integration of them fails, and `prov check` reports them.

### What phase 1 keeps general for it

OpenCode differs from Claude Code in ways that phase 1 already allows for. These phase 1 rules must not become specific to Claude Code:

| Phase 1 rule | Reason in OpenCode |
|---|---|
| A session starts at its first event | OpenCode has no session-start event. |
| `prov` cannot stop a second session; the worktree becomes contended | OpenCode gives no way to refuse a session. |
| The adapter decides how it gets the transcript | OpenCode has no transcript file. |
| The adapter decides when the sweep runs | OpenCode has no Stop event. |
| A session is stale after 24 hours without an event, not only when its process is gone | One OpenCode server lives longer than its sessions. |
| The agent identity can come from the config | OpenCode has no standard commit identity. |

### Sessions

OpenCode has no session-start event and no reliable session-end event. Reason: one OpenCode server can serve several sessions over a long time.

- `prov` starts a session at the first forwarded event of that session. It ignores a session whose directory is outside the repository.
- A child session (a subagent) is not a session of its own for `prov`. The plugin adds the parent ID from `ctx.session.get`, and `prov` handles an event of a child session as an event of its top-level parent. Reason: a child session would otherwise make the worktree contended.
- OpenCode sends no event when the user changes to a new session. The old session stays Open, so a second session in the same worktree makes the worktree contended. Archive the old session first with `prov session archive`.
- `prov start opencode` does not replace itself with OpenCode. It waits until OpenCode exits, then moves each Open session in that worktree to Closing.
- A session that was started without `prov start` becomes stale. Archive it with `prov session archive`.

`prov` does these actions for the forwarded events:

| Event | Action |
|---|---|
| First event of an unknown session | Record the session as Open. |
| `session.inbox.enqueued` | Update the last event time. |
| `session.model.selected`, `session.step.started` | Update the last event time and the model. |
| End of an agent tool call | Update the last event time. Run the sweep. |
| `session.deleted` | Move the session to Closing. |

### Model, sweep and archive

- Model: the model from each `session.model.selected` and `session.step.started` event.
- Sweep: at the end of each agent tool call, and in `pre-commit`. The agent name is `opencode`, which is also git-ai's name for the tool.
- Archive: the output of `opencode session export <id>`, as `session.json`, and the export of each child session, as `children/<child ID>.json`. Reason: the export of a session holds only the ID of a child session, not its messages. `prov` finds the child IDs in the tool metadata (`sessionID`) of each export, also in the exports of the children.

### Commit identity

- OpenCode has no standard commit identity. The adapter's default is `OpenCode <opencode@example.invalid>`; replace it in the config and keep it.
- The plugin gives the identity and the signing setting only to shell commands that the agent starts. A command that the human types in OpenCode (with `!`) keeps the human identity. Reason: one OpenCode server runs both kinds of command. The plugin also removes inherited `GIT_AUTHOR_*` values from every command.
- git-ai 1.7.6 recognizes no OpenCode identity. OpenCode commits depend fully on checkpoints and the sweep.
- OpenCode sets `OPENCODE_SESSION_ID` in the agent's commands. `prov` uses it as an agent environment variable, as it uses `CLAUDE_CODE_SESSION_ID`.

### Hooks and the plugin

The events are `session.inbox.enqueued`, `session.model.selected`, `session.step.started`, `session.deleted`, and the end of each agent tool call. They are installed in `.opencode/plugins/prov.ts`, which `prov init` writes with the identity from the config.

**OpenCode plugin.** The plugin forwards the listed events unchanged to `prov hook opencode event`. It adds the session's directory, parent ID and model from `ctx.session.get`. `prov` does all session logic. The plugin keeps one other job: it sets the agent identity on agent shell commands. Reason: that change must reach the command's environment before the shell starts, and a call to `prov` for each command adds a process start.

- `prov` embeds the plugin text. The payload type in the plugin is generated from the Rust type with `ts-rs`, so the two cannot disagree.
- OpenCode runs the plugin with its own Bun runtime. No TypeScript build step is necessary.
- The plugin forwards only the listed events. Reason: streaming events would start one `prov` process for each part.
- A plugin failure never stops OpenCode.

### Other additions in phase 2

- Protected paths: `.opencode/**`, `opencode.json` and `opencode.jsonc`.
- `prov doctor`: the OpenCode plugin has the expected content, and it sets `commit.gpgsign=false` and the agent identity.
- Config: `opencode` in `agents`, and an `[agent.opencode]` section for the identity.
- Crate `prov-adapters`: the OpenCode adapter, the embedded plugin, and the payload type for `ts-rs`.
- Test: a plugin test runs the generated OpenCode plugin in OpenCode's Bun (`BUN_BE_BUN=1 opencode`) with a mock plugin context and the real `prov` binary.
- Trace export: an OpenCode model that already has a provider stays unchanged.

### Test evidence

**OpenCode 2.2.6, tested 2026-11-11**

| Test | Result | Rule |
|---|---|---|
| `opencode session export` of a session with one subagent | The export holds the child session's ID in the tool metadata, not its messages. `opencode session list` shows only top-level sessions. | Archive step exports each child |
| `opencode session export <child ID>` | Works. The export has `parentID` and the child's messages. | Archive step exports each child |
| A session edits one file (git-ai's OpenCode plugin). Then an `agent-v1` checkpoint with tool `opencode`, the session ID and a different model name on a second file. Clean environment. | One session key for both files. Both files AI, no human lines. The note had the model of the `agent-v1` checkpoint. | Sweep joins the agent's session |

## Appendix: test evidence

These results are the basis of rules in this specification.

**git-ai 1.7.6, tested 2026-11-11**

| Test | Result | Rule |
|---|---|---|
| `git-ai bg status --repo` | 10 ms at the 95th percentile | Service check in hooks |
| `agent-v1` checkpoint, 1 file and 20 files | 7 to 8 ms at the 95th percentile | Sweep inside the 100 ms limit |
| `git status --porcelain` | 5 ms | Sweep inside the 100 ms limit |
| The same commands in a worktree on a 9p mount (2026-11-12) | Each `stat` 2 to 5 ms. `git status` 280 ms with 40 files and 490 ms with 55 files in deeper directories. `git rev-parse` 25 to 40 ms. Hooks: SessionStart 494 ms, UserPromptSubmit 194 ms, Stop with a sweep of 20 files 866 ms (median). The file lock, hard links, renames, symbolic links and the executable bit work. | Relative limit on a slow file system |
| A file written by an agent outside any checkpoint, committed by a human with no AI lines in the commit | Recorded as human | "Keeping attribution complete" |
| Replay with `known_human` and `agent-v1` stages | Notes had the expected session keys | `prov cp` procedure |
| Replay with an untracked stage first (`git-ai checkpoint human`) | The lines did not stay untracked. git-ai attached them to the nearby AI edit. | Every line must have an author |
| Deferred copy check, exact plan | Cleared in seconds | `prov cp` step 6 |
| Deferred copy check, three identical lines with different authors | Authors moved between the lines. Reported as a note. | Identical lines are compared as a group |
| Deferred copy check, plan changed to a wrong model | Failed | `prov cp` step 6 |
| `git-ai status --json --diff-only` before a commit | 0 lines for a new file. `known_human` lines counted as unknown. | No pre-commit gate |
| `git-ai show`, `git-ai blame` and `git-ai stats --json` with no service and no git-ai config | All three read the notes | `prov check` needs no service |
| `git-ai diff <commit> --json` on a commit with human and AI lines | Each added range has a `session_id` (AI) or a `human_id` (human) | `prov` reads JSON and has no note parser |
| `git-ai blame <file> --json` on a file with human and AI lines | Only the AI ranges are listed | `prov cp` inherits only AI sessions |
| `git merge --squash` of an agent commit, then commit | The squash commit got a note with the same session keys | A squash of agent commits is visible to `prov check` as AI sessions on a human commit |
| `git cherry-pick` of an agent commit | The new commit got a note with the same session keys | The same, for a cherry-pick |
| The same tool and session ID in a checkpoint in a different repository | The same session key | An inherited session keeps its key |
| Two file replays and one ordinary file in a single commit | Each replayed file had exactly its plan. The ordinary file was human. | A commit can hold several copies |

**git-ai 1.7.6, tested 2026-11-12**

| Test | Result | Rule |
|---|---|---|
| An `agent-v1` checkpoint of type `ai_agent` on `vendor/v.txt`, then a commit | `git-ai diff --json` listed no range for the file. `git-ai blame --json` listed no AI line. `git-ai stats --json` counted 0 added lines. | Gate item for a file with no ranges; copy result `ignored` |
| `git-ai effective-ignore-patterns` (not in the help text) | `*.lock`, `Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `go.sum`, `Gemfile.lock`, `poetry.lock`, `composer.lock`, `Pipfile.lock`, `shrinkwrap.yaml`, `*.generated.*`, `*.min.js`, `*.min.css`, `*.map`, `**/vendor/**`, `**/node_modules/**`, `**/__snapshots__/**`, `**/*.snap`, `**/*.snap.new`, `**/drizzle/meta/**`, and generated protobuf files | The same |
| `git-ai blame` with `--rev`, `--commit`, `--newest-commit`, `<rev> -- <file>` | All refused. Only the file in the working tree is read. | `prov cp` step 1, temporary clone |
| `git-ai blame <file> --json` in a `git clone --shared --no-checkout` of the source, after a fetch of `refs/notes/ai` and a checkout of the commit | The AI ranges of the file at that commit | `prov cp` step 1 |
| `git-ai blame --json` on a file with 0 lines | Fails ("Invalid line range") | `prov cp` does not run blame on an empty file |
| `git-ai await --timeout 60` in a test under load, repositories on a 9p mount | Once exited with an error after 6 seconds: "background service could not finish (0 metrics, 0 notes remaining)". Every note was written. Not reproduced in 36 parallel awaits or 96 commits. | The gate does not depend on `git-ai await` |
| An `ai_agent` checkpoint with a model on a file whose change already had a checkpoint with the model `unknown`, then a commit | The note kept `unknown`. With a change in another file in the same checkpoint, the note had the new model. | git-ai takes a model only from a checkpoint that records a change ("Agent model") |
| Replay of a file with a human and two AI sessions (two tools, two models), lines of each between the others | The note had exactly the plan | `prov cp` procedure |
| A file with no checkpoint and a file with an `ai_agent` checkpoint, in one commit with the agent identity and no Claude Code variables | Both files had the AI session as author | Taint is a `known_human` checkpoint (R-SWEEP-2) |
| A file with no checkpoint alone, in a commit with the agent identity, with and without `CLAUDE_CODE_SESSION_ID` | A session that git-ai invented, in both cases | "Sessions of an integration" |
| A file with no checkpoint and a file with an `ai_agent` checkpoint, in one commit with a human identity | The first file was untracked: an addition with no `session_id` and no `human_id` | The gate's untracked lines; this answers an open question of version 0.1 |
| A file with no checkpoint alone, in a commit with a human identity | `known_human` for the committer | The same |
| The same, with a `known_human` checkpoint on the first file before the commit | The first file was human. The gate listed it as doubtful. | Rejected as the taint checkpoint: the note would say human |
| The same, with an `ai_agent` checkpoint (tool `prov-unattributed`, a new conversation ID) on the first file, and no checkpoint of type `human` before it | The first file had the `prov-unattributed` session. The same in a commit by a human, with a second taint (a second session key), and in `git-ai blame` of a clone. | Taint checkpoint (R-SWEEP-2) |
| The same, with a checkpoint of type `human` on the file just before the `ai_agent` checkpoint | The `ai_agent` checkpoint recorded nothing: the `human` checkpoint had taken the content as it was | The taint checkpoint has no checkpoint of type `human` before it |

**git-ai 1.7.6, contract tests, 2026-11-12**

| Test | Result | Rule |
|---|---|---|
| `crates/prov/tests/gitai_contract.rs`, 10 tests | All pass. A checkpoint on a new file just before a commit counted for that commit in 20 of 20 commits. | Tested version 1.7.6 |
| A human line, then one character of it changed through an agent edit, in an agent commit | `git-ai diff --json`: the whole line is one AI addition (and one deletion). `git-ai blame --json` gives the line one session. `git-ai stats --json`: `ai_additions` 1, no other category. | README: attribution is per line, with no "mixed" category |
| A binary file with an `ai_agent` checkpoint, in an agent commit | `git-ai diff --json` lists no range for the file | README: binary files are not attributed |
| `git merge` of a branch, run from the Bash tool of Claude Code 2.1.325, with a `prepare-commit-msg` hook that writes its environment | The hook sees `CLAUDECODE` and `CLAUDE_CODE_SESSION_ID` (and other `CLAUDE_*` variables) | The merge gate refuses an agent |
| A human branch with an integration, brought into `main` as a forge does it: no hooks and no git-ai service; a merge commit, a squash, and a rebase. Test `forge_merge_methods_of_a_human_branch` | Merge commit: every note stays, `prov check` passes. Squash: one commit with no note, `prov check` passes although the commit has AI lines. Rebase: the agent commit is on the first-parent line, `prov check` fails | Checking: merge commits only for human branches |
| `git push` of a branch with notes to a bare repository, git-ai 1.7.6 with its service running; then `git clone` of that repository | The remote has `refs/notes/ai` after the push. The clone has `refs/notes/ai` and `refs/notes/ai-remote/origin` | Checking: notes reach the remote |
| git 2.53: `git diff -U0` of a pure insertion; `git blame --porcelain -L` past the end of the file, at line 0, and with two overlapping ranges; `git log --no-walk --format='%H %(trailers:key=Prov-Revises,valueonly)'` on several commits | The hunk header is `@@ -N,0 +M,k @@`, with `N` the line after which the lines go and `N = 0` at the start. A range past the end is clamped to the last line. `-L 0,1` fails ("invalid line number"), so the code asks for line 1. On an empty file, `-L 1,1` fails, so an empty old file is skipped by its blob ID. Overlapping ranges are merged; each line has one header with its line number. One `git log` call gives each commit with its trailer values. Tests `revise_an_integration` and `revise_two_integrations` | R-REV-1: insertions, earlier revisions and several targets |
| git-ai 1.7.6, own home, a repository with no remote: a checkpoint with an empty `allow_repositories`, with `*`, with a path entry; then with a remote and a matching and a non-matching URL pattern. Test `allow_repositories_matches_remote_urls_only` | Empty list: recorded, the commit gets a note. `*`: skipped with the message. A path entry: `git-ai config --add` refuses ("has no remotes configured. Add a remote first or use a glob pattern"). A matching pattern (`https://` form against an `scp`-form remote): recorded. A pattern that matches no remote: skipped | Requirements; Doctor checks |
| `prov discard` and `git merge` of the same branch, started together, one of them late by 0 to 250 ms, 60 rounds. Test `discard_racing_a_merge_keeps_the_evidence` | 35 rounds merged first, 25 discarded first. Every transcript hash in a merge commit had its archive. After a merge, the discard found no own commits and no records, and removed only the worktree and the branch | R-DISC-1; `ProvDiscard`: `EvidenceKept` |
| `prov discard` and an agent event of the same session (a resume), started together, 8 rounds, five runs. Test `discard_racing_a_resume` | The hook never failed. Most rounds the discard refused (the session was Open). When the discard won, the event made a new Open record with a known process ID for the worktree that step 3 then removed | R-DISC-1, accepted limit |
| `prov session archive` of an 8 MB transcript, killed with SIGKILL at 2/10 to 9/10 of the time of one uncut step. Test `an_archive_process_killed_during_its_write` | No file with the final name was ever cut off. Running the step again archived the session. A kill during the write left `<session>-<hash>.tar.tmp<pid>`, which nothing deletes | R-ARCH-1 |
| `git commit` of a file on an LFS-tracked path after `prov cp`, with LFS's hooks chained through `prov init`, git-lfs 3.7.1 | Before the fix, the commit took 60 seconds: the detached copy check inherited a pipe of `git-lfs filter-process`, and git waited for the filter. The check then found no note. With the detached process keeping no inherited descriptors: 66 ms, and the check agrees. Test `copies_from_a_source_with_git_lfs` | Detached processes |
| `prov cp` from a source with Git LFS, and from one with a clean filter; a destination filter that changes the bytes. Tests `copies_from_a_source_with_git_lfs`, `copies_the_blob_of_a_filtered_file` | `prov cp` copies the blob: the LFS pointer, or the cleaned content. In a destination with LFS, the object is missing and a clone shows the pointer. `git lfs install` refuses to replace prov's shims; `git lfs install --force` and `prov init` again chain LFS's hooks as `.local`. A destination filter gives a note, and the check agrees when the line count stays; a filter that changes the line count makes the check fail | Copying files |
| `prov cp` of 2,000 files of two lines (`copies_two_thousand_files`, run with `--ignored`) | `prov cp` 43 to 51 s. `git commit` 0.2 s. The note arrived after 9 s; the check ended after 66 s, because its `git-ai await` waited its full 60 s while the service worked on the checkpoints of the copy | Copying files |
| Two `claude -p` processes in one worktree, Claude Code 2.1.325, Haiku: one runs `sleep`, the other writes a file and commits | The second session's SessionStart hook warned, and the agent changed nothing. Told to commit anyway, `pre-commit` refused: "this worktree is contended. 2 sessions are open here". A later commit of the leftovers: the file that the contended session wrote kept its own session (its own git-ai hook recorded it); two files that sessions wrote after the contention ended, alone, were tainted | R-SWEEP-2, R-SWEEP-3 |
| `prov` hooks in the harness: session A, then B starts and ends, then A edits and stops | After B's SessionEnd the owner stays `null`; A's next event taints and makes A the owner. With the harness's `agent-v1` checkpoint, A's file keeps A. In the live run, git-ai's own `claude` checkpoint of the file was overridden by the taint at A's Stop | R-SWEEP-2 |
| A merge with an editor that stays open; the session of the branch resumed with `claude -p --resume` meanwhile, Claude Code 2.1.325 | The gate wrote the trailers before the editor opened. The resume returned the session to Open, and its end archived it again under a new hash. The merge commit names the first hash; that archive stayed. `post-commit` kept the record (R-INT-2). `prov check` passed | `ProvIntegration` P4, R-INT-2 |
| One agent commit, git-ai 1.7.6: a binary file and a changed binary file with `ai_agent` checkpoints, a mode change only, a symbolic link with an `ai_agent` checkpoint, a submodule gitlink, a deleted file, and a file that only loses a line | `git-ai diff --json`: no added range for the binary files and the mode change; one AI range for the symbolic link; one untracked range for the gitlink; only a deletion hunk for the deleted file. `git diff-tree --raw --numstat -z` gives the raw records, then the numstat records, in one process: `-` for a binary file, `0 0` for the mode change. Test `binary_files_mode_changes_and_deletions_have_no_added_ranges` | The merge gate: files doubtful as a whole |
| The full test suite in a Podman container (Ubuntu 26.04, local disk), many tests at once | `git-ai checkpoint` returns when the service has received the checkpoint, not when it has applied it. In 2 of 3 runs, the sweep's checkpoint in `pre-commit` was applied about 50 ms after the commit was recorded, and the commit's lines went to a session that git-ai invented. With `git-ai await` at the end of `pre-commit`: 0 of 6 runs. A checkpoint captures the file's content when it is received: two AI stages of one file, with no wait between them, were attributed correctly. | `pre-commit` and `prov approve` wait for git-ai after their checkpoints |

**git 2.53 and git-ai 1.7.6, premises of `prov revise` (proposed), tested 2026-11-13**

The setup: a human branch and an agent branch with `ai_agent` checkpoints. The agent branch merged the human branch midway (a clean merge). The integration `M` had one conflict, which the human resolved with new text.

| Test | Result | Rule |
|---|---|---|
| `git commit --template <file>`, the template with an empty first line, a `Prov-Revises` trailer and comment lines; editor that changes nothing | "Aborting commit; you did not edit the message." Exit code 1. With a subject written: the commit has the subject and the trailer, and no comment lines. | `prov revise` step 6 |
| `git blame HEAD -L 3,4 -L 7,8 --porcelain -- <file>` | One call blames all the ranges. Ranges out of order and ranges that overlap are accepted, and each line is listed once. | One blame call for each file |
| `git blame` of the merged file at `M` | Lines from a clean merge: the agent commit that wrote them. New text in a conflict resolution: `M`. A line that the resolution kept from one side: the commit of that line. | Merged lines are lines blamed to `M^1..M^2` or to `M` |
| `git log M^1..M^2`, where the agent branch had merged the human branch midway | The agent commits and the agent's merge commit. Not the human commit that the agent branch merged. Its line was blamed to that human commit. | The same |
| A human commit after `M`, no checkpoint: one AI line changed, one AI line and one conflict line deleted | `git-ai diff --json`: the changed line is one addition with the committer's `human_id`. Deletions have no author. No session. | No checkpoint before the revise commit |
| The same, with an `ai_agent` checkpoint on another file that is not staged | The same result. The unstaged AI file does not change the note. | The same |
| The same, with a `known_human` checkpoint on the file first | The same result as with no checkpoint | The same |
| `prov check <base>..HEAD` after an integration and a human commit that changes and deletes merged AI lines, with a `Prov-Review-Of: <M>` trailer, the name proposed then (temporary test, removed). `prov check` does not read this trailer, so its name does not change the result. | `2 commit(s) checked, 0 failure(s)` | `prov check` accepts a revise commit with no change |

**git 2.53, tested 2026-11-11**

| Test | Result | Rule |
|---|---|---|
| `git pull` with `mergeoptions = --no-ff`, branch only behind | A merge commit. With `pull.ff = only`, a fast-forward. | `prov init` sets `pull.ff` |
| `prepare-commit-msg` during `git merge --no-verify` | The hook runs, and `MERGE_HEAD` exists | Gate in `prepare-commit-msg` |
| `prepare-commit-msg` refuses a merge | The merge stays in progress with the changes staged | "When the gate refuses" |
| An agent branch replaces a hook file in a tracked hooks directory, then is merged | git ran the branch's copy of the hook. The gate did not run. The same happened for a squash commit. | Hooks directory outside every worktree |
| `git merge --ff-only` | `post-merge` and `reference-transaction` ran | Fast-forward warning |
| `git branch -d` on a pushed branch that is not merged | Deleted, with a warning | `prov finish` never deletes a branch |

**git 2.53, tested 2026-11-13**

| Test | Result | Rule |
|---|---|---|
| MERGE_HEAD of another worktree: `git merge --no-commit` of an agent branch in one worktree; from another worktree, `git rev-parse --path-format=absolute --git-path MERGE_HEAD` in each worktree of `git worktree list --porcelain` | The file of the merging worktree was found and held the tip of the agent branch | R-DISC-1, merge check |
| reference-transaction during a merge commit: a `reference-transaction` hook checks for MERGE_HEAD when the update of the human branch is committed. Once with `git merge`, once with `git merge --no-commit` and then `git commit` | MERGE_HEAD existed in both cases. It was gone when `post-commit` ran. | R-DISC-1: no moment has MERGE_HEAD gone and the branch not yet moved |
| `git branch -D` of a branch that a worktree has checked out | Refused: "cannot delete branch ... used by worktree" | R-DISC-1: the worktree goes before the branch |
| `git worktree remove` of a worktree with a changed file, an untracked file and an ignored file | Refused without `--force`. With `--force`, the directory was removed. | "Discarding" uses `--force` |

**Claude Code 2.1.322, tested 2026-11-11**

| Test | Result | Rule |
|---|---|---|
| `claude -p`, then `claude -p --resume <id>` | The resumed session had the same session ID and the same transcript file | "Resuming" |
| A session edits one file (own git-ai hooks). Then an `agent-v1` checkpoint with tool `claude` and the session ID on a second file. Clean environment, outside any agent session. | One session key for both files. Both files AI, no human lines. | Sweep joins the agent's session |
| The same test, with a different model name in the `agent-v1` checkpoint | One session key. The note had the model of the `agent-v1` checkpoint. The agent's own hooks alone recorded `unknown`. | "Agent model" |
| Session folders next to transcript files | Large tool results are in `<session ID>/tool-results/`, not in the transcript | Archive step stores the directory |
| A session with one subagent | The subagent's messages are in `<session ID>/subagents/agent-<id>.jsonl`. The transcript holds only the prompt and the result. | Archive step stores the directory |
| `CLAUDE_PID` in the environment of a hook: `prov start claude -- -p ...`, 2026-11-12 | The `agent_pid` that the hook recorded was the process ID of `claude` itself (`prov start` replaces itself with `claude`). The same after `prov start --resume`, with the new process ID. | R-SESS-3 |
| A live run: `prov start claude -- -p <task> --model haiku`, the agent writes one file and commits; then `prov start --resume <id> -- -p <task>`; then `git merge` on `main` | Commit identity and `Prov-Session` correct. Each run archived after the process exited (generations 2 and 4, two archive files). The gate passed with no doubtful line; the merge got the hash of the second archive; the record was deleted. `prov check <base>..<tip>` and `prov doctor --deep` passed. | "Starting a session", "Resuming", "Archive step", the merge gate |
| `prov session archive` during a live run: `prov start claude -- -p <task>`, the task starts with `sleep 15`, the close 4 s after the start | The session went to Closing (generation 2), and the command waited for the agent process. 16 s later the agent's next event opened the session again (generation 3, R-SESS-1), and its SessionEnd closed it (generation 4). The archive of generation 4 was the whole transcript; the step of generation 2 wrote nothing. The agent's commit while the session was Closing had no `Prov-Session` trailer (no occupant); the gate found the session through the note, and `prov check` of the range passed. | R-SESS-1, R-SESS-2, R-SESS-3 |
| The transcript of the live run, compared with the archive that `prov` made after `claude` exited | Byte for byte the same, and unchanged four hours later. One run. | The transcript does not grow after the agent exits (R-SESS-3) |
| The same run: the model in the note | `unknown` for the first commit, `claude-haiku-5-5` for the second (the record had the model from the first Stop). The transcript had the model in the assistant message of the `git commit` call, before the call. | "Agent model" |

## Appendix: model evidence

The models are in `tla/`. Each rule with an ID of the form `R-<AREA>-<n>` has a model, except R-ARCH-1, which a Rust test covers.

**TLC 2.19 (tla2tools 1.7.4), checked 2026-11-12**

| Model | Property | Constants | Result | Rule |
|---|---|---|---|---|
| ProvSweep | SweepSound | 2 sessions, 2 files | Holds, 1,933 states | R-SWEEP-1, R-SWEEP-2, R-SWEEP-3, R-SWEEP-5 |
| ProvSweep, no taint | SweepSound | same | Fails in 6 steps: A writes, A crashes, B starts, B commits. The sweep at SessionEnd does not prevent it. | R-SWEEP-2 |
| ProvSweep, background writes | SweepSound | same | Fails in 6 steps: accepted limit | R-SWEEP-4 |
| ProvSessions | ArchivedIsCurrent | 1 session, work ≤ 2, generations ≤ 5 | Holds, 691 states | R-SESS-1, R-SESS-2, R-SESS-3 |
| ProvSessions, with fairness | ClosingEnds (liveness): a Closing session whose agent has exited becomes Archived, Failed or Open | same | Holds, 691 states. Without fairness it fails (`reach-stuck-closing`), so the property is not trivial. The 5-minute rule is the calculated state `EffSt`. | Archive step, the 5-minute rule |
| ProvSessions, "still Closing" with no generation | ArchivedIsCurrent | same | Fails in 8 steps: end, resume, work, end again, and the first archive process writes | R-SESS-2 |
| ProvSessions, no wait for exit | ArchivedIsCurrent | same | Fails in 6 steps: close of a live session | R-SESS-3 |
| ProvSessions, process ID not known | ArchivedIsCurrent | same | Fails in 6 steps: accepted limit | R-SESS-3 |
| ProvIntegration | NoNeededDeletion | 2 named sessions, 1 other, hash versions ≤ 3 | Holds, 7,668 states | R-INT-1, R-INT-2 |
| ProvIntegration | CommitIsComplete | same | Fails in 4 steps: an edited message or a resume before the commit; accepted limit | the accepted limit of R-INT-2 |
| ProvIntegration, 0.1 cleanup | NoNeededDeletion | same, and variants | Fails in 4 to 5 steps: another session's record, edited message, resume | R-INT-2 |

**TLC 2.19 (tla2tools 1.7.4), checked 2026-11-13**

| Model | Property | Constants | Result | Rule |
|---|---|---|---|---|
| ProvDiscard | EvidenceKept: every transcript hash in a commit on a human branch has its archive. NoOpenDeleted. | 2 sessions, hash versions ≤ 3, 1 merge | Holds, 1,694 states | R-DISC-1 |
| ProvDiscard, gate without the lock | EvidenceKept | same | Fails in 8 steps: the gate reads the records, discard deletes them and the archives, the commit names the archive | R-DISC-1 |
| ProvDiscard, no merge check | EvidenceKept | same | Fails in 8 steps: the gate has passed, discard deletes, the commit lands | R-DISC-1 |
| ProvDiscard, no ref check under the lock | EvidenceKept | same | Fails in 8 steps: the commit lands between the check and the lock | R-DISC-1 |

Each model also has `reach-*` configs that must fail. They show that sweeps, taint, contention, archiving, discarded archive results, commits and deletions occur in the model. One liveness property is checked: `ClosingEnds` in `ProvSessions`. The other properties are safety properties only.

The models were trimmed on 2026-11-12, with the same results before and after: in `ProvSweep`, an idle session is a crash followed by a resume (7,629 states became 1,933), and the sweep at SessionEnd has no switch (it serves complete attribution, not `SweepSound`). In `ProvSessions`, the switch for R-SESS-1 (never off) and the "archive always writes" variant (the 0.1 text, which the code never had) were removed.
