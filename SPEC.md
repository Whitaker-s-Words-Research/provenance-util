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
| Agent worktree | A git worktree on an agent branch. It holds one Open session at a time. |
| Session | One run of an agent, identified by the agent's session ID. |
| Contended worktree | An agent worktree with more than one Open session that is not stale. See "Binding rules". |
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

- `prov` changes git state only through the `git` command. It writes inside `.git` only in `.git/provenance/` and the hook shims in the hooks directory.
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
- An agent hook returns in less than 100 ms at the 95th percentile.
- The repository must be allowed by git-ai's `allow_repositories` setting. git-ai matches that setting against remote URLs only. A repository with no remote is not allowed. In a repository that is not allowed, git-ai ignores checkpoints and prints only a message.
- A session is integrated from the clone that ran it. Reason: live state and transcripts are local to a clone.
- Minimum git version: 2.31. Reason: `GIT_CONFIG_COUNT`.
- Compatibility baseline: git-ai 1.7.6. `git-ai await` is marked beta in that version, and `prov` depends on it.
- The hook events in this specification were tested with Claude Code 2.1.322.

## Sessions, worktrees and branches

Each agent session works in its own worktree, never in a human worktree. Reason: parallel sessions need separate files and branches, and the sweep is valid only where nothing else changes files.

### Starting a session

`prov start <agent>` creates a worktree on a new agent branch and starts the agent in that worktree. This command is optional. A person can also run `git worktree add` and start the agent. The agent's SessionStart hook binds the session to the worktree in both cases.

```text
prov start <agent> [options] [-- <agent arguments>]
```

| Option | Default | Effect |
|---|---|---|
| `--name <slug>` | 4 random hex characters | The `<name>` part of the branch name. Letters, digits and `-` only. |
| `--branch <name>` | `agent/<agent>/<YYYYMMDD>-<name>` | Full branch name. It must match `agent/**`. |
| `--from <ref>` | the branch that is checked out | Start point of the new branch. |
| `--path <dir>` | `../<repo>.worktrees/<agent>-<YYYYMMDD>-<name>`, from the base directory | Location of the worktree. `<repo>` is the name of the base directory. |
| `--no-launch` | off | Create the worktree, print its path, and do not start the agent. Use this for agents in an IDE. |
| `--resume <session>` | none | Open the worktree of that session and start the agent with its resume option. If the worktree was removed, `prov` creates it again on the session's branch. If the session has no record, also give `--branch`. |

`prov` names the branch, not the session. Reason: the agent assigns the session ID at SessionStart, after the branch exists. The branch and worktree names are fixed conventions, not config keys. The date is the local date. It makes `git branch --list 'agent/*'` sort by time. If the branch exists, `prov start` stops with an error.

### Binding rules

- An agent worktree holds one Open session at a time. Only an Open session that is not stale occupies a worktree. A Closing session does not. A stale session does not: a new session in its worktree works normally. The stale session stays Open, and it must be closed before its branch integrates.
- After a session ends, a new session can bind to the same worktree and branch.
- `prov` cannot stop a second session. Reason: a SessionStart hook cannot stop Claude Code, and not every agent has a start event. When a second session starts in an occupied worktree, the worktree becomes contended:
    - `prov` records the second session as Open.
    - For Claude Code, the SessionStart hook prints an instruction to stop. Claude Code adds that text to the session context.
    - `prov` runs no sweep in a contended worktree, and `pre-commit` refuses every commit there.
    - The worktree stays contended until only one session occupies it. A session ends by itself, or the human runs `prov session close`.
    - Lines without a checkpoint from that time become doubtful lines. `prov` does not attribute them to one of the sessions.
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
| Open | The first event records the session. A resumed session returns to Open. |
| Closing | SessionEnd occurs. A detached process starts the archive step. |
| Archived | The archive step stores the transcript. |
| Failed | The archive step cannot finish. The record holds the error text. A session that is Closing for 5 minutes with no live archive process also becomes Failed. |

- An Open session is stale when no agent event arrives for 24 hours, or when the agent process does not exist. Stale is calculated when the state is read. It is not stored.
- To find the agent process, `prov` uses `CLAUDE_PID` for Claude Code and the parent process for other agents.
- `prov session close <id>` moves an Open session to Closing. For a Failed session, it runs the archive step again.
- `prov session drop <id>` deletes the record of a session. It keeps the transcript archive.
- `prov doctor` reports stale and Failed sessions.
- An integration deletes the records of the sessions on its branch. See "Sessions of an integration".

### Resuming

A resumed Claude Code session keeps its session ID. An Archived, Failed or stale session can be resumed. It returns to Open. A session that is already integrated has no record. When it is resumed, the first event makes a new record with the same session ID. Each archive of a session is a separate file, so the hash in an earlier integration stays correct.

### Agent model

The sweep checkpoint needs the model of the session. The adapter finds it, and `prov` keeps the latest model in the session's live state:

- Claude Code: the `model` field of the SessionStart payload. At each Stop, the model of the latest assistant message in the transcript replaces it.
- No known model: `unknown`.

The sweep checkpoint joins the git-ai session of the agent's own hooks, because the tool name and the conversation ID are the same. A git-ai session has one model, and the model of the sweep can become the model of the whole session. In a test with Claude Code, the agent's own hooks recorded the model `unknown`, and the note then had the model that the sweep gave. `prov` must therefore give the correct model.

### Archive step

1. Build the archive through the adapter. The adapter decides how it gets the transcript. The archive is a tar file that `prov` builds:
    - Claude Code: the file at the transcript path from the hook payload, as `transcript.jsonl`, and every file of the directory `<session ID>/` next to the transcript, with its relative path. Reason: Claude Code stores large tool results in `<session ID>/tool-results/` and the messages of subagents in `<session ID>/subagents/`, not in the transcript.
    - The entries are sorted by name, and each entry has modification time 0, owner 0 and mode 0644, so the same files always give the same bytes.
2. Calculate SHA-256 of the tar bytes, before compression. The hash therefore covers the transcript, the tool results and the subagent sessions. To check an archive: decompress it and hash the result.
3. Write the archive, gzip-compressed, to `<logs_dir>/<agent>/<session>-<first 12 hex characters of the hash>.tar.gz`. `prov` never overwrites an archive file.
4. Set the state to Archived. If a step fails, set the state to Failed and store the error text.

Every session that ends is archived, also a session that made no commit.

### Finishing

`prov finish <session or branch>` runs `git worktree remove` and keeps the branch. The branch is a record of the session.

- `prov finish` refuses while a session in the worktree is Open or Closing.
- `--override` removes the worktree in that case too. It first moves each Open session of the worktree to Closing.
- `--force` goes to `git worktree remove`.
- `prov finish` keeps the session records. Reason: a later integration of the branch needs the transcript hash. The records are deleted only by the integration or by `prov session drop`.
- `prov` never deletes a branch. Reason: `git branch -d` checks the branch's upstream, not the human branch that it started from, so it can delete a branch that is not integrated.

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
| `pre-commit` | Refuse a commit by an agent on a branch that does not match `agent/**`. In an agent worktree: refuse if the worktree is contended; do the service check; do the sweep if one session is Open. Do the copy guard for each copied file. |
| `prepare-commit-msg` | For an integration: run the merge gate, then add the integration trailers. For a commit in an agent worktree with one Open session: add `Prov-Session`. For all commits: add the `Prov-Approved` trailers of `--human` and `--ai` approvals, and the `Copied-From` trailers. |
| `post-commit`, `post-merge` | After an integration: delete the live state of its sessions. After a commit with copied files: start the deferred copy check. After a fast-forward of a human branch: see "Squash, fast-forward, rebase and cherry-pick". |

In an agent worktree with no Open session, `pre-commit` runs no sweep. Changes there have no checkpoint, so they become doubtful lines and the human decides at the gate. Reason: `prov` must not attribute a change to a session that did not make it.

There is no `pre-merge-commit` shim. Reason: `MERGE_HEAD` does not exist when that hook runs, and `--no-verify` skips it. `prepare-commit-msg` has `MERGE_HEAD` and runs also with `--no-verify`.

### Limits of local hooks

Hooks are advisory for an agent. An agent can commit with `--no-verify`, write to the hooks directory, or change `core.hooksPath`. The controls are these:

- the merge gate, which runs from the hooks directory during integration
- `prov check`, which does the same check on a human branch without local hooks
- `prov doctor`, which checks the hooks directory and the shim contents

### No pre-commit gate

`prov` does not check line authorship before an ordinary commit. Only integrations are gated. Reason: git-ai's counts before a commit are not reliable. In a test, `git-ai status --json --diff-only` counted 0 lines for a new file, and it counted `known_human` lines as unknown.

## Keeping attribution complete

git-ai attributes a line through the checkpoints around the change. git-ai applies its recovery steps to a committed line with no checkpoint. If no step matches, git-ai records the line as `known_human` for the committer, also in an agent commit. `prov` prevents the four causes:

| Cause | Control |
|---|---|
| The agent commit identity is not recognized by git-ai | Use the agent identity in "Commit identity". |
| A process that the agent started writes after the tool call ends | Sweep. In an agent worktree with one Open session, every change is the session's change. `prov` runs `git-ai checkpoint agent-v1` with type `ai_agent` on the changed files. The adapter decides when the sweep runs. For Claude Code, it runs at each Stop and in `pre-commit`. There is no sweep at the end of a session. Reason: a sweep has an effect only on the next commit, and the agent changes nothing after its last Stop. The agent name is `claude`, which is also git-ai's name for the tool. The conversation ID is the session ID. The model comes from "Agent model". |
| The git-ai service is not available | `git-ai bg status --repo <worktree>` gives exit code 1 when the service is not available. A checkpoint then exits 0 and records nothing. At SessionStart, `prov` shows a warning and runs `git-ai bg start`. In an agent worktree, `pre-commit` refuses the commit. On a human branch, `pre-commit` shows a warning. |
| The git-ai integration of an agent is not installed | `prov doctor` checks the integration of each configured agent. |

- An agent must commit where the git-ai service is reachable. It must not commit inside a sandbox that blocks the service. Reason: `pre-commit` refuses the commit when the service check fails.
- The human does not edit files in an agent worktree.
- Processes that continue after SessionEnd, for example a development server, are not swept. Stop them before `prov finish`.

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

- The integration fails if one of these sessions is not Archived, or has no record. Close an Open session with `prov session close` first.
- The integration fails while a session is Open in a worktree whose branch tip is in the incoming range.
- After the integration, `prov` deletes the records of its sessions. It also deletes the records of other sessions on that branch that no trailer or note names. Their transcript archives stay.

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
| `Prov-Version` | each integration | `<version>+<build SHA>` | yes |
| `Copied-From` | a commit with copied files | `<source>@<commit>:<path>`. `<source>` is the remote URL of the source repository, or its absolute path if it has no remote. | one for each copied file |

## The merge gate

No doubtful line reaches a human branch without a human approval. `prov` reads each agent commit with `git-ai diff <commit> --json`. An added line is doubtful when one of these is true:

- No range in the output gives it a session or a human (untracked).
- A range gives it a human. Reason: in an agent commit, this label comes from git-ai's fallback, not from a person.
- Its agent commit has no note.
- A range gives it a session that no `Prov-Session` trailer names and that has no record. See "Sessions of an integration".

A merge commit in the incoming range is not read with `git-ai diff`. Reason: git-ai shows a merge as the difference from its first parent, which repeats the lines of the merged commits. Instead, each path that the merge commit changed relative to every parent is doubtful. This is what the author of the merge wrote, for example an agent that merged the human branch into its branch and resolved a conflict. A clean merge has no such path.

A change to a protected path is also doubtful. The default protected paths are `.provenance.toml`, `.claude/**` and `.github/workflows/**`. Reason: an agent could change these files on its own branch to weaken the controls.

The gate runs in `prepare-commit-msg` for each integration:

1. It reads the config with `git show HEAD:.provenance.toml`. If the binary version does not satisfy `required_version`, the integration fails.
2. It runs `git-ai await --timeout 60`. On a timeout, the integration fails. Run the commit again.
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
- `prov approve` asks for a typed `yes` at a terminal. For a protected path, it first shows the diff of that path between the human branch (`HEAD` of the merge) and the incoming tip.
- A merge approval exists only in the merge message of the merge in progress. `git merge --abort` discards it. `prov` keeps no other record of it, and you cannot approve before you start the merge.
- `--human` and `--ai` are for lines whose author git-ai cannot know. Examples: code that you paste from an AI tool in a browser (`--ai`), and the output of a generator that you ran (`--human`). The approval stays in live state with the hash of the file content. A change to the file cancels it.
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

## Copying files: `prov cp`

```text
prov cp --from <repo> [--rev <rev>] <path>... <dest> [options]
```

`prov cp` copies files, as they are at a commit, into the current repository, with an author for every line. `<repo>` is the path of a local clone. `<path>` is relative to the root of the source. A `<path>` can be a directory: `prov cp` then copies each file under it that the source commit tracks. The default `<rev>` is `HEAD`.

- Only a human can run it. It refuses in every agent worktree, and when an agent environment variable is set.
- It refuses if a destination file exists. To replace a file, see "Replacing the authorship of a file".
- It preserves the executable bit.
- It copies a binary file or a symbolic link unchanged, with no line authorship, and shows it in the plan.

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

1. Read each file from the source commit, never from a working tree.
2. Check that git-ai allows the repository and that no destination file exists.
3. Write and resolve the plan. Store it in `.git/provenance/copies/<id>.json`.
4. Replay each file. Write the file in stages, with one checkpoint after each stage:
   1. The human lines, then `git-ai checkpoint known_human --editor prov --editor-version <version>`.
   2. For each AI session: an `agent-v1` checkpoint of type `human`, that session's lines, then an `agent-v1` checkpoint of type `ai_agent`.

   The last stage makes the file equal to the source, byte for byte.
5. Stage the files. The human commits them. The commit can hold several copies and other changes. `pre-commit` refuses the commit if a copied file is different from the copied bytes. `prepare-commit-msg` adds one `Copied-From` trailer for each copied file.
6. Deferred check. `post-commit` starts a detached process. The process waits for the note for a maximum of 60 seconds. Then it compares the authors of each copied file with the plan, by content. In each group of identical lines, the set of authors must agree with the plan.
   - Agreement for every file: `prov` deletes the plan.
   - Authors moved between identical lines: `prov doctor` shows a note.
   - Disagreement, or no note after 60 seconds: the plan stays, and `prov doctor` reports a failure for that file. A later copy of the same path that agrees clears the failure.

Session records and transcripts stay in the source repository. The copy has the line authorship and the `Copied-From` trailer. To find the transcript of an inherited session, follow `Copied-From` to the source.

### Replacing the authorship of a file

No public git-ai command changes an existing note, and git-ai attributes only the lines that a commit changes. To correct the authorship of a committed file, remove it and copy it again:

1. `git rm <path>`, and commit.
2. `prov cp --from . --rev <commit before the removal> <path> <path>` with `--all`, or with a plan that you edit. Commit.

**Cost**: one commit of the history does not have the file, and `git blame` then shows the second commit for every line. Do this only when an authorship is wrong or a copy check failed.

## Trace export (optional)

`prov trace export [<range>]` writes Agent Trace records as JSON Lines, one record per commit with a note. The default range is the branch that is checked out. It reads each commit with `git-ai diff <commit> --json`.

- The schema is Agent Trace 0.1.0, pinned in `vendor/agent-trace/0.1.0/` with its CC BY 4.0 notice. `prov` supports only this version.
- `prov` validates each record against the pinned schema before it writes the record.
- `version` is `"0.1"`. Reason: the schema requires the pattern `^[0-9]+\.[0-9]+$`.
- `id` is a version 5 UUID of the commit SHA, in a fixed namespace that the `prov` source defines.
- `timestamp` is the committer date of the commit.
- `vcs` is type `git` with the commit SHA as the revision.
- `tool` is name `prov` with the `prov` version.
- AI lines map to contributor type `ai` with `model_id`. Human lines map to `human`. Untracked lines map to `unknown`.
- `model_id` has the form `<provider>/<model>`. The adapter adds the provider when the note has only the model: `anthropic/` for Claude Code. A model that already has a provider stays unchanged. If the model is `unknown`, the record has no `model_id`.
- Two exports of the same range give identical bytes.

## Commands

| Command | Run by | Effect |
|---|---|---|
| `prov init [--agent <name>]... [--force]` | Human, once in each clone | Writes `.provenance.toml`, the shims in the hooks directory and the Claude Code settings. Sets `merge.ff` and `pull.ff`. Adds `logs_dir` to `.gitignore` if it is inside a worktree. Refuses to overwrite a changed file without `--force`. |
| `prov doctor [--deep]` | Human | Checks the items in "Doctor checks". Exit code 1 if one check fails. |
| `prov check [<agent branch> \| <base>..<tip>]` | Human, CI | See "Checking". |
| `prov start <agent>` | Human | See "Starting a session". |
| `prov hook <adapter> <event>` | Shims only | Handles an agent or git event. |
| `prov session list` / `show <id>` / `close <id>` / `drop <id>` | Human | Shows sessions, closes one, or deletes the record of one. |
| `prov finish <session or branch> [--override]` | Human | See "Finishing". |
| `prov approve <path>...` | Human | See "Approval". |
| `prov cp --from <repo> [--rev <rev>] <path>... <dest>` | Human | See "Copying files". |
| `prov trace export [<range>]` | Human | See "Trace export". |

### Doctor checks

`prov doctor` checks the local repository. It is not for CI; CI runs `prov check <base>..<tip>`.

- `.provenance.toml` parses, and the binary version satisfies `required_version`.
- The hooks directory is outside every worktree. Each shim has the expected content.
- `merge.ff` is `false`, and `pull.ff` is `only`.
- The agent environment sets `commit.gpgsign=false` and the agent identity, in `.claude/settings.json`.
- `.provenance.toml` and the agent settings files are committed.
- git-ai is installed, its version is the compatibility baseline or later, its service is available, and it allows the repository. With `--deep`, `git-ai debug` passes.
- The git-ai integration of each configured agent is installed.
- No session is stale or Failed. No worktree is contended. No copy check failed.

## Configuration

`.provenance.toml` is in the repository root and is tracked by git.

```toml
required_version = "=0.1.0"        # exact version. The merge gate and prov check refuse on a mismatch.
agents = ["claude"]                # the agents in use
logs_dir = "llm-logs"              # relative to the base directory
protected = [".provenance.toml", ".claude/**", ".github/workflows/**"]

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
- Hooks read the config from their own checkout. The merge gate reads it from `HEAD`, and `prov check` reads it from the first parent of each integration.
- `required_version` is enforced by the merge gate and by `prov check`. On a mismatch, all other hooks show a warning and continue. Reason: an agent branch that started before a version change still has the old value in its checkout.

## Live state

All files are in `.git/provenance/` in the common git directory, so every worktree sees them. All writes use one advisory file lock, `lock`.

| File | Content | Deleted when |
|---|---|---|
| `sessions/<id>.json` | state, agent, worktree, branch, model, transcript path, archive file and hash, error text (Failed), agent PID, start time, last event time, `prov` version | the branch of the session is integrated, or `prov session drop` |
| `copies/<id>.json` | the plan, the blob ID of each copied file, the check result of each file | the copy check agrees for every file. A failed file is cleared when a later copy of the same path agrees. |
| `approvals/<key>.json` | path, choice (`human` or `ai`), content hash | the approval becomes a trailer. Merge approvals are not stored here. |

## Build and release

- Releases are tagged and built from `Cargo.lock` with `--locked`. The Rust toolchain is pinned in `rust-toolchain.toml`.
- The release notes give the SHA-256 of each binary.
- `Prov-Version` names the version and the git SHA of the build.
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
- Attribution tests run git-ai in a repository with an allowed remote, outside any agent session and with a clean environment. Reason: an agent session's own git-ai hooks attribute changes made by test commands to that session.
- A copy test replays several files with different plans into one commit and checks each file against its plan.
- A property test checks that `prov` refuses a trailer value with a line break.
- A latency test fails if an agent hook (`prov hook <adapter> <event>`) takes more than 100 ms at the 95th percentile.
- A trace test exports one range twice and compares the bytes.
- CI runs on Linux and macOS.

## Alternatives considered

| Option | For | Against | Result |
|---|---|---|---|
| Rust binary (this design) | No runtime in the target repository. Fast start for hooks that run at each prompt. Same language as git-ai. | Slower to change than a script. | Chosen |
| Python package installed with `uv tool install` | Fastest to build. | Needs Python and uv on every machine. About 100 ms start time for each hook. | Fallback if the Rust build stalls |
| Shell scripts copied into each repository | No build or release process. | Copied and edited for each project. Hard to test. Differences between macOS and Linux in every script. | Rejected |

Rust is chosen because hooks run at every prompt and at session end under a time limit, and because a binary with a pinned version is easy to cite.

## Open questions

1. Where is the `prov` repository published?
2. How does an untracked line appear in `git-ai diff --json`? The gate defines it as an added line with no session and no human. A test could not make such a line: in git-ai 1.7.6, the recovery steps gave every test line an author.

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
- OpenCode sends no event when the user changes to a new session. The old session stays Open, so a second session in the same worktree makes the worktree contended. Close the old session first with `prov session close`.
- `prov start opencode` does not replace itself with OpenCode. It waits until OpenCode exits, then moves each Open session in that worktree to Closing.
- A session that was started without `prov start` becomes stale. Close it with `prov session close`.

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
| A file written by an agent outside any checkpoint | Recorded as human | "Keeping attribution complete" |
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

**git 2.53, tested 2026-11-11**

| Test | Result | Rule |
|---|---|---|
| `git pull` with `mergeoptions = --no-ff`, branch only behind | A merge commit. With `pull.ff = only`, a fast-forward. | `prov init` sets `pull.ff` |
| `prepare-commit-msg` during `git merge --no-verify` | The hook runs, and `MERGE_HEAD` exists | Gate in `prepare-commit-msg` |
| `prepare-commit-msg` refuses a merge | The merge stays in progress with the changes staged | "When the gate refuses" |
| An agent branch replaces a hook file in a tracked hooks directory, then is merged | git ran the branch's copy of the hook. The gate did not run. The same happened for a squash commit. | Hooks directory outside every worktree |
| `git merge --ff-only` | `post-merge` and `reference-transaction` ran | Fast-forward warning |
| `git branch -d` on a pushed branch that is not merged | Deleted, with a warning | `prov` never deletes a branch |

**Claude Code 2.1.322, tested 2026-11-11**

| Test | Result | Rule |
|---|---|---|
| `claude -p`, then `claude -p --resume <id>` | The resumed session had the same session ID and the same transcript file | "Resuming" |
| A session edits one file (own git-ai hooks). Then an `agent-v1` checkpoint with tool `claude` and the session ID on a second file. Clean environment, outside any agent session. | One session key for both files. Both files AI, no human lines. | Sweep joins the agent's session |
| The same test, with a different model name in the `agent-v1` checkpoint | One session key. The note had the model of the `agent-v1` checkpoint. The agent's own hooks alone recorded `unknown`. | "Agent model" |
| Session folders next to transcript files | Large tool results are in `<session ID>/tool-results/`, not in the transcript | Archive step stores the directory |
| A session with one subagent | The subagent's messages are in `<session ID>/subagents/agent-<id>.jsonl`. The transcript holds only the prompt and the result. | Archive step stores the directory |
