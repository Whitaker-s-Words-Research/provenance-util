# Configuration

## `.provenance.toml`

`prov init` writes it in the repository root. It is tracked by git, and it is a protected path: a change to it on an agent branch needs your approval at the gate.

```toml
required_version = "=0.1.0"   # the prov version; the merge gate and prov check refuse on a mismatch
agents = ["claude"]           # the agents in use
logs_dir = "llm-logs"         # where transcript archives go, relative to the base directory
protected = [".provenance.toml", ".claude/**", ".github/workflows/**", "CLAUDE.md", "AGENTS.md"]
known_gaps = []               # commits with no git-ai note that you accept

[agent.claude]                # optional; replaces the adapter's defaults
command = ["claude", "--model", "opus"]
```

| Key | Meaning |
|---|---|
| `required_version` | The version requirement for the `prov` binary. The merge gate and `prov check` refuse on a mismatch. Other hooks only warn, because an agent branch that started before a version change still has the old value. |
| `agents` | The agents in use. Phase 1 supports `claude`. |
| `logs_dir` | The directory of the transcript archives. Not tracked by git. |
| `protected` | Paths whose every change is a doubtful item at the gate. gitignore patterns: `*` does not match `/`, `**` matches across directories. |
| `known_gaps` | Commit IDs (7 or more hex characters) with no git-ai note that you accept instead of repairing them. `prov doctor` then lists them as notes, not failures, and `prov check` gives no warning for them. |
| `[agent.<name>]` | Replaces `author`, `email` or `command` of the agent's adapter. |

Relative paths resolve against the base directory: the repository root in a normal clone, or the directory that holds a bare repository with linked worktrees.

Where the config is read from:

- Hooks read it from their own checkout.
- The merge gate reads it from `HEAD`, the human branch before the merge. During a merge, the file in the worktree already has the agent's changes.
- `prov check <base>..<tip>` reads it from the first parent of each merge.

## Fixed conventions

These are not config keys.

| Convention | Value |
|---|---|
| Agent branches | `agent/**` |
| Name of a new agent branch | `agent/<agent>/<YYYYMMDD>-<name>` |
| Worktrees | `../<repo>.worktrees/`, from the base directory. For one clone, set another directory with `git config prov.worktreesDir <dir>`, for example a local disk when the repository is on a slow mount. |
| Transcript archives | `<logs_dir>/<agent>/<session>-<12 hex characters of the hash>.tar.gz` |
| Agent names | `claude` (adapter `claude-code`) |
| Agent commit identity | `Claude <noreply@anthropic.com>` for Claude Code |
| Stale session | 24 hours without an event, or no agent process |
| Live state | `.git/provenance/` in the common git directory |

Keep the Claude Code identity. git-ai recognizes it: a committed line with no checkpoint then goes to Claude. With any other identity, git-ai gives such a line to the committer as a human.

## What `prov init` changes

| Where | What |
|---|---|
| `.provenance.toml` | Written if missing or if it does not parse. |
| `.claude/settings.json` | The hooks SessionStart, UserPromptSubmit, Stop and SessionEnd, each running `prov hook claude-code <event>`. `env` with the agent identity and `commit.gpgsign=false`, so agent commits are never signed with your key. Other keys are kept. |
| The hooks directory | Shims for `pre-commit`, `prepare-commit-msg`, `post-commit` and `post-merge`. Each is `exec prov hook git <name> "$@"`. An existing hook is renamed to `<name>.local` and runs after `prov`'s work. |
| git config of the repository | `merge.ff = false` and `pull.ff = only`. These apply to every branch. |
| `.gitignore` | `logs_dir`, if it is inside the repository. |

The shims and the git config are not tracked. Run `prov init` in every new clone.
