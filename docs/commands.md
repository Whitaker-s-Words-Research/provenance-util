# Commands

Every command that changes something accepts `--dry-run`. Every command that shows something accepts `--json`, for scripts. `prov <command> --help` shows the options.

| Command | Run by | What it does |
|---|---|---|
| [`prov init`](#prov-init) | A person, once in each clone | Sets up the config, hooks, agent settings and git options. |
| [`prov doctor`](#prov-doctor) | A person | Checks the setup and reports problems. |
| [`prov start`](#prov-start) | A person | Creates a worktree on a new agent branch and starts the agent. |
| [`prov check`](#prov-check) | A person, CI | Applies the rules of the merge gate without a merge. |
| [`prov approve`](#prov-approve) | A person | Records a decision about the authorship of files. |
| [`prov revise`](#prov-revise) | A person | Commits your changes to the lines of a merge of agent work. |
| [`prov session`](#prov-session) | A person | Lists, shows, archives or drops sessions. |
| [`prov finish`](#prov-finish) | A person | Removes an agent worktree and keeps the branch. |
| [`prov discard`](#prov-discard) | A person | Deletes an agent branch that will not be merged, with its worktree, records and archives. |
| [`prov cp`](#prov-cp) | A person | Copies files from another repository with an author for every line. |
| [`prov completions`](#prov-completions) | A person | Turns on tab completion. |
| [`prov trace export`](#prov-trace-export) | A person | Writes Agent Trace records. |
| `prov hook <adapter> <event>` | Hook shims only | Handles an agent or git event. Do not run it by hand. |

## `prov init`

```text
prov init [--agent <name>]... [--force]
```

Writes `.provenance.toml`, the hook shims and `.claude/settings.json`, sets `merge.ff = false` and `pull.ff = only`, and adds `logs_dir` to `.gitignore`. See [Getting started](getting-started.md#setting-up-a-repository).

- `--agent <name>`: an agent in use. Default: the agents in the config, or `claude`.
- It keeps an existing `.provenance.toml` that parses. `--force` writes `.provenance.toml` again from the defaults, which removes your own settings such as extra protected paths and `known_gaps`; `git diff` shows them. It also replaces a hook or a settings value that has other content.
- The hooks directory must be outside every worktree, so that an agent branch cannot replace the hooks that run during its own merge. `prov init` accepts a `core.hooksPath` only if it is an absolute path outside every worktree.

## `prov doctor`

```text
prov doctor [--deep]
```

Checks the clone. Exit code 1 if a check fails. See [Troubleshooting](troubleshooting.md) for the checks and how to fix each one. `--deep` also runs `git-ai debug`. `prov doctor` is not for CI; CI runs `prov check <base>..<tip>`.

The only change it makes: it removes the record of an agent worktree that git no longer lists.

## `prov start`

```text
prov start <agent> [options] [-- <agent arguments>]
```

| Option | Default | Effect |
|---|---|---|
| `--name <slug>` | 4 random hex characters | The `<name>` part of the branch name. Letters, digits and `-`. |
| `--branch <name>` | `agent/<agent>/<YYYYMMDD>-<name>` | The full branch name. It must match `agent/**`. |
| `--from <ref>` | the branch that is checked out | The start point of the new branch. |
| `--path <dir>` | `prov.worktreesDir`, or `../<repo>.worktrees/` | The directory that holds the worktree. The worktree's name comes from the branch. A leading `~` is the home directory. |
| `--no-launch` | off | Create the worktree, print its path, and do not start the agent. |
| `--resume <session>` | none | Open the worktree of that session (create it again if it was removed) and start the agent with its resume option. If the session has no record, also give `--branch`. |

Arguments after `--` go to the agent. The `[agent.<name>] command` key in the config replaces the default start command.

## `prov check`

```text
prov check [<agent branch> | <base>..<tip>] [--into <branch>]
```

Changes nothing. Exit code 1 if a check fails.

| Form | Use |
|---|---|
| `prov check` | Lists each agent branch that is not merged into the target, with its commits, sessions and the number of doubtful lines. |
| `prov check <agent branch>` | Lists what the gate will ask about: sessions that are not Archived, doubtful ranges, changed protected paths. Exit code 0 means a plain `git merge` passes now. |
| `prov check <base>..<tip>` | Checks commits that are already on a human branch. For CI; see [Checking in CI](ci.md). |

The target is the branch that is checked out. `--into <branch>` names another. In an agent worktree or with a detached `HEAD`, `--into` is necessary.

## `prov approve`

```text
prov approve (--merge | --human | --ai [--agent <tool>] [--model <model>]) <path>...
```

| Option | When | Effect |
|---|---|---|
| `--merge` | During a merge, after the gate stopped | The lines keep their git-ai attribution. Adds `Prov-Approved: <path> ack` to the merge message. |
| `--human` | Before your own commit | Marks your changes to the file as yours (a `known_human` checkpoint). |
| `--ai` | Before your own commit | Marks your changes to the file as AI-written. `--agent` and `--model` default to `unknown`. |

Asks for a typed `yes` at a terminal; with no terminal it refuses. `--dry-run` shows the items without a prompt. Refuses in an agent worktree and when an agent environment variable is set. See [Workflow](workflow.md#the-merge-gate).

## `prov revise`

```text
prov revise [--of <merge>]... [--dry-run]
```

Commits your staged changes to the lines of an integration, with a `Prov-Revises: <merge>` trailer for each integration that it revises. git opens the editor with the trailer and comment lines; you write the message. An unchanged message stops the commit.

- The target is the latest merge with a `Prov-Session` trailer on the first-parent line from `HEAD`. `--of <merge>` names another; it must be such a merge and an ancestor of `HEAD`. Give `--of` more than once for a change that crosses the work of several merges.
- Refuses unless, for each target, the staged changes change or delete at least one merged line, or insert lines next to one. A merged line is a line that `git blame` gives to the merge, to a commit that it merged, or to an earlier revision of the merge. Lines inserted between two other lines, and the lines of a new file, do not count.
- Refuses when nothing is staged, and when a staged file also has changes that are not staged.
- Prints, for each target, how many merged lines the changes change or delete and how many lines they insert next to merged lines, then how many other lines they change or delete. `--dry-run` stops there.
- Refuses in an agent worktree and when an agent environment variable is set.

`prov check` treats the commit as any other human commit. See [Workflow](workflow.md#revising-the-merged-work).

## `prov session`

```text
prov session list
prov session show <id>
prov session archive <id>
prov session drop <id>
```

- `list` and `show` print the session records: state, worktree, branch, model, archive.
- `list` shows the start of each session ID, as much as tells the sessions apart. Every command that takes `<id>` accepts it.
- `list` and `show` also show the title that Claude Code gave the session. They read it from the `ai-title` records in the transcript. Claude Code does not document these records, so a new version can drop them; the title is then missing. No rule depends on the title.
- `archive` moves an Open session to Closing and archives it, after the agent process exits. For a Failed session, it runs the archive step again.
- `drop` deletes the record of a session. The transcript archive stays. Use it for a record that `prov doctor` says is left over after a merge. To throw away a branch and everything of its sessions, use [`prov discard`](#prov-discard).

## `prov finish`

```text
prov finish <session or branch> [--override] [--force]
```

Removes the worktree and keeps the branch and the session records. Refuses while a session there is Open or Closing; `--override` closes those sessions first. `--force` goes to `git worktree remove`. Prints the command that deletes the branch. See [Workflow](workflow.md#finishing).

## `prov discard`

```text
prov discard <session or branch> [--yes]
```

Deletes the work of an agent branch that will not be merged: the worktree with its uncommitted changes, the branch, the session records and the transcript archives. Without `--yes` it only prints what it would delete. It keeps the agent's own transcripts, the git-ai notes and a remote branch of the same name. It refuses while a session of the branch is Open or Closing, while a merge of the branch is in progress, and when another ref names one of its sessions or copied a file from it. Refuses in an agent worktree and when an agent environment variable is set. See [Workflow](workflow.md#discarding).

## `prov cp`

```text
prov cp --from <repo> [--rev <rev>] <path>... <dest> [options]
prov cp --apply <plan file>
```

| Option | Effect |
|---|---|
| `--from <repo>` | The path of a local clone of the source. |
| `--rev <rev>` | The source commit. Default: `HEAD` of the source. |
| `--all human\|ai` | The author of every line. The source's notes are ignored. |
| `--missing human\|ai` | The author of every line that has none from the source. |
| `--agent <tool>`, `--model <model>` | With `ai`: the tool and the model. Default: `unknown`. |
| `--session <id>` | With `--all ai`: the conversation ID. Default: the plan ID. |
| `--plan <file>` | Write the plan to a file and stop. |
| `--apply <file>` | Apply a plan file. |

See [Copying files](copying.md).

## `prov completions`

```text
prov completions [--shell bash|zsh|fish] [--install]
```

Prints the line that turns on tab completion. `--install` appends it to `~/.bashrc`, `$ZDOTDIR/.zshrc` or `$XDG_CONFIG_HOME/fish/config.fish`, unless it is there. The default shell is `$SHELL`. Completion offers branch names and session IDs from the repository of the current directory.

## `prov trace export`

```text
prov trace export [<range>]
```

Writes one Agent Trace 0.1.0 record per commit with a note, as JSON Lines. The default range is the branch that is checked out. See [The record](record.md#agent-trace-export).
