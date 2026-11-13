# Commands

Every command that changes something accepts `--dry-run`. Every command that shows something accepts `--json`, for scripts. `prov <command> --help` shows the options.

| Command | Run by | What it does |
|---|---|---|
| [`prov init`](#prov-init) | A person, once in each clone | Sets up the config, hooks, agent settings and git options. |
| [`prov doctor`](#prov-doctor) | A person | Checks the setup and reports problems. |
| [`prov start`](#prov-start) | A person | Creates a worktree on a new agent branch and starts the agent. |
| [`prov check`](#prov-check) | A person, CI | Applies the rules of the merge gate without a merge. |
| [`prov approve`](#prov-approve) | A person | Records a decision about the authorship of files. |
| [`prov session`](#prov-session) | A person | Lists, shows, closes or drops sessions. |
| [`prov finish`](#prov-finish) | A person | Removes an agent worktree and keeps the branch. |
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
- It keeps an existing `.provenance.toml` that parses. `--force` replaces a file, a hook or a settings value that has other content.
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
| `--path <dir>` | `../<repo>.worktrees/` | The directory that holds the worktree. The worktree's name comes from the branch. |
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

## `prov session`

```text
prov session list
prov session show <id>
prov session close <id>
prov session drop <id>
```

- `list` and `show` print the session records: state, worktree, branch, model, archive.
- `close` moves an Open session to Closing and archives it, after the agent process exits. For a Failed session, it runs the archive step again.
- `drop` deletes the record of a session. The transcript archive stays. Use it for a record that `prov doctor` says is left over after a merge.

## `prov finish`

```text
prov finish <session or branch> [--override] [--force]
```

Removes the worktree and keeps the branch and the session records. Refuses while a session there is Open or Closing; `--override` closes those sessions first. `--force` goes to `git worktree remove`. Prints the command that deletes the branch. See [Workflow](workflow.md#finishing).

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
