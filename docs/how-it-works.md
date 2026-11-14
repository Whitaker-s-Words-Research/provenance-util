# How it works

This page explains what `prov` does behind the commands. The full rules, with their reasons, are in [SPEC.md](../SPEC.md).

## Ground rules

- `prov` never writes git-ai data. It sets authorship only through git-ai's public checkpoint commands, and git-ai writes the notes. It reads authorship only through git-ai's JSON output (`git-ai diff`, `blame` and `stats`).
- `prov` changes git state only through the `git` command. Inside `.git` it writes only `.git/provenance/` and the hook shims.
- A fact is stored in one place: git-ai notes hold line authorship, commit trailers hold session records, and live state holds only work in progress.

## Why git-ai needs help

git-ai attributes a change through the checkpoints around it. Its Claude Code hooks place one before and one after each tool call. A committed line with no checkpoint gets a default author that depends on the commit, not on what happened (git-ai 1.7.6):

| Commit author | Other lines of the commit | A line with no checkpoint becomes |
|---|---|---|
| The agent identity | lines of an AI session | a line of that session |
| The agent identity | none of an AI session | a line of a session that git-ai invents |
| A person | lines of an AI session | untracked |
| A person | none of an AI session | `known_human`, for the committer |

`prov` removes the causes of lines with no checkpoint:

| Cause | What `prov` does |
|---|---|
| The agent's commit identity is one git-ai does not recognize | `prov init` sets `Claude <noreply@anthropic.com>` for the agent's commands. |
| A process the agent started writes after the tool call ends | The [sweep](#the-sweep). |
| The git-ai service is down, so checkpoints record nothing | SessionStart warns and starts it. `pre-commit` refuses every commit, on every branch, while it is down. |
| git-ai's Claude Code integration is not installed | `prov doctor` checks it. |

## Hooks

| Hook | Jobs |
|---|---|
| Claude Code SessionStart | Record the session as Open and bind it to the worktree. Warn about a session outside an agent worktree or in a contended one. Check the git-ai service. |
| Claude Code UserPromptSubmit | Record the event. |
| Claude Code Stop | Update the model from the transcript. Sweep. |
| Claude Code SessionEnd | Sweep, mark the session Closing, and start a detached archive process. |
| `pre-commit` | Refuse an agent commit on a human branch. Check the git-ai service. In an agent worktree: refuse if contended, else update the model and sweep, then wait for git-ai. Check copied files. |
| `prepare-commit-msg` | For a merge of agent commits: run the merge gate and add the trailers. In an agent worktree: add `Prov-Session`. Add `Prov-Approved` and `Copied-From` trailers. Refuse a squash of agent commits. |
| `post-commit`, `post-merge` | After a merge: delete the records of the merged sessions. After a copy: start the copy check. After a fast-forward onto agent commits: warn. |

The git hooks live in the hooks directory, outside every worktree. Otherwise, during a merge git would run the agent branch's copy of a hook, and a branch could switch off its own gate.

Hooks are advisory for an agent: it can commit with `--no-verify` or change the hooks. The controls that count are the merge gate, which runs when a person merges, `prov check` in CI, and `prov doctor`, which checks the hooks.

## The sweep

In an agent worktree with one occupant, every change is that session's change: nobody else works there. At each Stop, at SessionEnd and in `pre-commit`, `prov` runs `git status` and makes a git-ai `agent-v1` checkpoint of the changed files for that session, with its model. The checkpoint joins the git-ai session of the agent's own hooks, because the tool name (`claude`) and the conversation ID are the same.

A checkpoint covers only the changes since the one before it. Lines already recorded by the agent's own hooks keep their attribution.

## Taint

`prov` cannot see who made a change, only that a file changed. So for each agent worktree it remembers the *owner*: the session it last saw as the only occupant. Every hook in the worktree first works out the occupants. If they are not exactly the owner, for example a session crashed and a new one started, or two sessions run at once, the changes on disk at that moment are *tainted*.

Tainted changes get a checkpoint from a tool named `prov-unattributed`, with model `unknown` and a new conversation ID. No session record or trailer names it, so its lines are doubtful at the gate, and you decide. A tainted line that you approve still says that an AI wrote it, in a session nobody knows.

A worktree with no occupant gets no sweep. Its changes are tainted when the next session arrives.

## Session states

| State | Entered when |
|---|---|
| Open | The first event of the session. Any later event (except SessionEnd) also returns a Closing, Archived or Failed session to Open, because the agent is still working. |
| Closing | SessionEnd, or `prov session close`. A detached process starts the archive step. |
| Archived | The archive step stored the transcript. |
| Failed | The archive step could not finish, or the session stayed Closing for 5 minutes with no live archive process. |

*Stale* is not stored: an Open session is stale when no event arrived for 24 hours, or its agent process is gone.

Each record has a generation, which goes up each time the session enters Open or Closing. An archive process belongs to one generation, and changes the state only if the session is still Closing with that generation. Without this, a session that ends, is resumed, works and ends again could be marked Archived with the first, shorter transcript.

## The archive step

1. Wait until the agent process exits, if `prov` knows its process ID (`CLAUDE_PID`). An archive made while the agent works would miss its last work.
2. Build a tar file of the transcript and the session's directory: sorted entries, time 0, owner 0, mode 0644, so the same files always give the same bytes.
3. Hash the tar bytes with SHA-256.
4. Write the gzip-compressed archive to a temporary file, then give it its final name with an operation that fails if the name exists. An existing archive with the same hash counts as success; one with another hash is a failure. `prov` never overwrites an archive.
5. Set the state to Archived, or to Failed with the error.

## The model

The sweep checkpoint needs the session's model. For Claude Code it comes from the SessionStart payload, and at each Stop and in `pre-commit` the model of the latest assistant message in the transcript replaces it. git-ai takes a model only from a checkpoint that records a change, so when every edit went through the agent's own hooks, the note can still say `unknown`. The `Prov-Model` trailer on the merge records the model either way.

## Live state

All files are in `.git/provenance/` in the common git directory, so every worktree sees them. One advisory file lock covers every read-modify-write.

| File | Content | Deleted when |
|---|---|---|
| `sessions/<id>.json` | the session record: state, generation, worktree, branch, model, archive and hash, agent process ID, times | a merge names the session with its hash, `prov session drop`, or `prov discard` |
| `worktrees/<key>.json` | the owner of one agent worktree | `prov finish` or `prov discard`, or `prov doctor` finds the worktree gone |
| `copies/<id>.json` | a copy plan and its check results | the copy check agrees |
| `approvals/<key>.json` | a `--human` or `--ai` approval with the file hash | the approval becomes a trailer |
