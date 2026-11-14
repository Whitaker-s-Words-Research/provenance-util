# Workflow

`prov` divides a repository into two kinds of branch:

- **Agent branches** match `agent/**`. Agents commit only here.
- **Human branches** are all other branches: `main`, and any feature branch that a person works on. Only people commit to them.

Agent work reaches a human branch only through a merge that a person makes on their own machine. That merge is the review.

## Sessions and worktrees

Each agent session works in its own git worktree, on an agent branch. Sessions never share files, so several can run at once.

### Starting

```sh
prov start claude --name parser-tests
prov start claude --from feature/parser          # start from another branch
prov start claude --no-launch                    # create the worktree only, for an agent in an IDE
prov start claude -- --model opus                # arguments after -- go to the agent
```

`prov start` is optional. You can also run `git worktree add` on an `agent/...` branch and start Claude Code there. The agent's SessionStart hook binds the session to the worktree either way.

- The branch is `agent/<agent>/<YYYYMMDD>-<name>`. Without `--name`, the name is 4 random hex characters. `--branch` sets the full name.
- The worktree is in `../<repo>.worktrees/`. `--path <dir>` puts it in another directory; its name still comes from the branch: `agent/claude/20261112-71fe` gives `claude-20261112-71fe`.
- If the branch exists, `prov start` stops.

### Rules for sessions

- **One session in one worktree.** `prov` cannot stop a second session, but the worktree then becomes *contended*: Claude Code is told to stop, `prov` sweeps nothing there, and every commit there is refused until one of the sessions ends. Changes from that time become doubtful lines at the gate.
- **Agents do not work in a human worktree.** A session in your checkout of `main`, or on any branch that is not `agent/**`, gets a warning and no record. An agent commit there is refused. If you commit that work yourself, `prov check` fails in CI.
- **Do not edit files in an agent worktree yourself.** `prov` attributes every change in the worktree to the session that occupies it.
- **Stop background processes when a session ends.** A process that keeps writing in the worktree, such as a development server, is not swept, and the next session there would take its changes as its own. `prov session close`, `prov finish` and `prov start --resume` list the processes that still run in the worktree.

### Ending, resuming and closing

When Claude Code exits, the session goes to Closing, and a detached process archives its transcript into `llm-logs/`. The session is then Archived. See [How it works](how-it-works.md#session-states).

```sh
prov session list                        # every session with a record, and its state
prov session show <id>
prov start claude --resume <id>          # resume a session in its worktree
prov session close <id>                  # end a session that did not end by itself
```

- A resumed Claude Code session keeps its ID. It returns to Open, and the next archive is a new file.
- If the worktree of a resumed session was removed, `prov start --resume` creates it again on the session's branch.
- `prov session close` is for a session that crashed or was left open. It waits for the agent process to exit before it archives. For a Failed session, it runs the archive step again.

A session that gets no event for 24 hours, or whose agent process is gone, is *stale*. It no longer occupies the worktree, but it must be closed before its branch can merge. `prov doctor` lists stale and Failed sessions.

## Merging

Merge from a human worktree:

```sh
git merge agent/claude/20261111-parser-tests
```

`prov init` sets `merge.ff = false`, so a plain `git merge` always makes a merge commit and the gate always runs. Only merge commits are permitted: squash, rebase, cherry-pick and fast-forward of agent commits onto a human branch are not. See [what happens if you do](#what-is-not-permitted).

**Merge where the session ran.** Session records and transcripts are local to a clone, so the person whose clone ran a session also merges it.

### Before the merge

```sh
prov check                                      # agent branches that are not merged into this branch
prov check agent/claude/20261111-parser-tests   # the items the gate will ask about
prov check <branch> --into main                 # compare against another target branch
```

`prov check <agent branch>` exits 0 only if a plain `git merge` passes the gate now.

### The merge gate

The gate runs in `prepare-commit-msg` for every merge that brings in agent commits. It:

1. Reads `.provenance.toml` from `HEAD` (the human branch before the merge, not the agent's copy) and checks `required_version`.
2. Waits for git-ai to write the notes of recent agent commits.
3. Checks the sessions. Every session that made a commit must be Archived. Close an Open session with `prov session close` first.
4. Lists the doubtful items. Each needs an approval.

An item is **doubtful** when:

- git-ai gives a line no author (untracked), or gives it a human. In an agent commit, a human label comes from git-ai's fallback, not from a person.
- An agent commit has no git-ai note.
- A line belongs to a session that no trailer names and that has no record. git-ai invents such a session, for example after `git commit --no-verify` skipped the sweep.
- git changed a file that git-ai lists no lines for, because git-ai ignores the path (`vendor/`, lock files, `*.min.js` and others). The whole file is one item.
- A merge commit on the agent branch changed a path relative to every parent, for example a conflict that the agent resolved.
- A protected path changed. The defaults are `.provenance.toml`, `.claude/**`, `.github/workflows/**`, and `CLAUDE.md` and `AGENTS.md` in any directory, because an agent could change them to weaken the controls.

### When the gate stops

git leaves the merge in progress, with the changes staged. The gate prints each open item and the next steps:

```sh
prov approve --merge notes.txt src/vendor/lib.js
git commit                       # the gate runs again
```

`prov approve --merge` asks for a typed `yes` at a terminal. For a protected path, it first shows the diff. It writes a `Prov-Approved: <path> ack` trailer into git's merge message; the lines keep their git-ai attribution.

To stop instead, run `git merge --abort`. That discards the approvals too.

### After the merge

The merge commit carries one trailer group for each session, with the hash of its transcript. See [The record](record.md). `post-commit` then deletes the records of the sessions that the commit names. The transcript archives stay.

If you edit the merge message in the editor and remove a trailer group, the record of that session stays, and `prov check <base>..<tip>` reports the commit. Undo the merge and make it again.

### Revising the merged work

You cannot edit an agent branch. To correct the agent's work, change it on a human branch after the merge, and commit with `prov revise`. After each merge, `prov` prints a hint with the ID of the merge.

```sh
git add src/parse.py      # your changes to the merged lines
prov revise               # git opens the editor; write the message
```

`prov revise` is optional. It refuses unless your staged changes change or delete at least one line that came in with the merge. Lines that you only add do not count. git opens the editor with a `Prov-Revises: <merge>` trailer, and you write the message. The default target is the latest merge of agent work on the first-parent line; `--of <merge>` names another. A merge can have several revisions. See [`prov revise`](commands.md#prov-revise).

### What is not permitted

| You do | What happens |
|---|---|
| `git merge --squash` of agent commits | `prepare-commit-msg` refuses the commit. Run `git reset --hard` to stop. |
| A fast-forward onto agent commits | No hook can refuse it. `post-merge` warns and prints `git reset --keep ORIG_HEAD`. |
| A rebase or cherry-pick of agent commits | `prov check` reports the commits in CI. |
| An octopus merge that contains agent commits | Refused. |
| A merge on a forge, such as a GitHub pull request | `prov check` reports it in CI. Protect your human branches on the forge so that agent branches cannot be merged there. |

`prov init` also sets `pull.ff = only`, so `git pull` on a human branch fast-forwards or stops instead of making a merge commit.

### Feature branches

The same rules apply to every human branch. You can work on `feature/x`, start agent branches from it with `--from feature/x` or by checking it out, and merge them back into it through the gate. A later merge of `feature/x` into `main` is a merge of human commits: the gate does not run again, and `prov check <base>..<tip>` checks the integrations inside it.

## Your own commits

`prov` does not gate an ordinary commit by a person. git-ai attributes your edits as usual. Two cases need help, because git-ai cannot know who wrote the lines:

```sh
prov approve --ai snippet.py --model gpt-x    # code you pasted from an AI tool in a browser
prov approve --human generated/schema.rs      # the output of a generator that you ran
git commit
```

- These apply to a complete file: every change in it since the last checkpoint. They cannot mark part of the changes.
- The approval is kept with a hash of the file. A change to the file before the commit cancels it.
- The commit gets a `Prov-Approved: <path> ai` or `Prov-Approved: <path> human` trailer.
- To change the authorship of lines that are already committed, see [Replacing the authorship of a file](copying.md#replacing-the-authorship-of-a-file).

Only a person can approve. `prov approve` refuses in an agent worktree and when an agent environment variable is set.

## Finishing

```sh
prov finish agent/claude/20261111-parser-tests     # or a session ID
```

- Removes the worktree and keeps the branch and the session records. A later merge of the branch needs the transcript hash.
- Refuses while a session in the worktree is Open or Closing. `--override` closes such sessions first.
- `--force` is passed to `git worktree remove`, for a worktree with uncommitted changes.
- Warns about processes that still run in the worktree.
- Prints the command that deletes the branch. `prov` never deletes a branch itself: `git branch -d` checks the branch's upstream, not the human branch it came from, so it can delete a branch that is not merged.
