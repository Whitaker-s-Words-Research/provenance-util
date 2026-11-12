# prov

`prov` keeps a record of what AI agents contribute to a git repository, and makes a human approve it before it reaches the main branch.

> **Status: design only.** `prov` is specified but not built yet. There is no release to install. This README describes how it will work. The full rules are in the specification, [SPEC.md](SPEC.md).

## What it does

When AI agents and people both commit to a repository, three questions come up later:

- Which lines did an agent write, and which did a person write?
- Which agent session produced this change, and where is its transcript?
- Who decided that this change could go into the main branch?

`prov` answers them with plain git and [git-ai](https://usegitai.com):

| Question | Where the answer lives |
|---|---|
| Who wrote each line | git-ai's notes on each commit |
| Which session made each commit | A `Prov-Session` trailer on the commit |
| Where the transcript is | An archive on your disk, with its hash in the merge commit |
| Who approved it | The merge commit that a person made, with `Prov-Approved` trailers |

`prov` stores almost nothing of its own. The record is in your git history, so anyone can read it with `git log`.

## How it works

1. **Each agent session gets its own worktree and branch.** Agents commit only on `agent/...` branches. Sessions never share files, so several can run at once.
2. **A person merges the agent branch.** The merge is the review. When you run `git merge`, the merge gate checks the incoming commits.
3. **The gate stops on anything doubtful.** A line is doubtful when git-ai cannot say who wrote it. A change to a protected file, such as the agent's own settings, is also doubtful. You approve each item, or you stop the merge.
4. **The merge commit carries the record.** It names each session and the hash of its transcript.

A human owns the responsibility for what goes into the main branch. The same rule applies to every branch that is not an agent branch (`agent/**`): you can work on a feature branch, start agent branches from it, and merge them back into it through the same gate. For that reason, `prov` supports only local merges. It does not support the merge button of a forge such as GitHub, and it never will.

## Requirements

- git 2.31 or later
- git-ai 1.7.6 or later, with the repository in its `allow_repositories` setting
- Linux or macOS
- Claude Code as the agent. OpenCode is planned for a second phase.

## A first session

Set up the repository once:

```sh
prov init --agent claude
git add .provenance.toml .claude/settings.json
git commit -m "Set up prov"
prov doctor
```

Start an agent in its own worktree:

```sh
prov start claude --name parser-tests
```

This creates the branch `agent/claude/<date>-parser-tests`, a worktree for it, and starts Claude Code there. The agent works and commits as usual. When you end the session, `prov` archives its transcript.

See what is waiting, then merge it:

```sh
prov check                                        # agent branches not merged yet
prov check agent/claude/20261111-parser-tests     # what the gate will ask about
git merge agent/claude/20261111-parser-tests
```

If the gate finds doubtful items, it stops and lists them. The merge stays in progress:

```sh
prov approve --merge notes.txt     # shows the item and asks for a typed "yes"
git commit                         # finishes the merge
```

To give up instead, run `git merge --abort`.

Remove the worktree when you are done. The branch stays as a record.

```sh
prov finish agent/claude/20261111-parser-tests
```

## What the record looks like

A merge commit after two sessions on one branch:

```text
Merge branch 'agent/claude/20261111-parser-tests'

Prov-Session: claude:5c1e2b9a-...
Prov-Transcript-SHA256: 9f2c...
Prov-Session: claude:a71d04e3-...
Prov-Transcript-SHA256: 41b8...
Prov-Approved: notes.txt ack
Prov-Version: 0.1.0+<build SHA>
```

To list every integrated session:

```sh
git log --first-parent --format='%h %s%n%(trailers:key=Prov-Session)' main
```

To see who wrote each line of a file, use git-ai:

```sh
git-ai blame src/parse.py
```

## Commands

| Command | What it does |
|---|---|
| `prov init` | Sets up a repository: config file, git hooks, agent settings. Run it once in each clone. |
| `prov doctor` | Checks that the setup is intact and reports sessions that need attention. |
| `prov start <agent>` | Creates a worktree on a new agent branch and starts the agent there. |
| `prov check` | Lists agent branches that are not merged yet. |
| `prov check <agent branch>` | Shows what the merge gate will ask about, before you merge. |
| `prov check <base>..<tip>` | Checks commits already on the main branch, and on the feature branches merged into it. This is the command for CI. |
| `prov approve <path>...` | Records your decision about a file. See "Approving" below. |
| `prov session list` / `show` / `close` / `drop` | Shows sessions, ends one that did not end by itself, or deletes the record of one. |
| `prov finish <branch>` | Removes an agent worktree and keeps the branch. |
| `prov cp` | Copies files from another repository, with an author for every line. |
| `prov trace export` | Writes the authorship record in the Agent Trace format. Optional. |

Every command that changes something accepts `--dry-run`. Every command that shows something accepts `--json`.

## Approving

`prov approve` has three uses. Only a person can run it; it refuses inside an agent session.

| Option | When you use it | Effect |
|---|---|---|
| `--merge` | During a merge, after the gate stopped | You accept the lines as they are. |
| `--ai` | Before your own commit | Marks your changes to the file as AI-written. Use it for code that you pasted from an AI tool in a browser. |
| `--human` | Before your own commit | Marks your changes to the file as yours. Use it for the output of a generator that you ran. |

`--ai` and `--human` apply to a complete file. They cannot mark a part of your changes.

## Copying files from another repository

`prov cp` copies files and keeps track of who wrote each line:

```sh
prov cp --from ../other-repo src/parse.py src/parse.py
prov cp --from ../other-repo src/ src/              # a directory
```

It first makes a plan: each file, with an author for every range of lines.

- Lines that an AI session wrote in the source keep that session.
- `--missing human` or `--missing ai` sets the author of all other lines.
- `--all human` or `--all ai` sets the author of every line and ignores the source's record. Use it when the source has no git-ai record.
- If some lines still have no author, `prov cp` opens the plan in your editor, as `git commit` does for a message. You fill in the authors and save.

Then you commit. The commit gets a `Copied-From` trailer for each file, which names the source repository, commit and path.

The transcripts of copied sessions stay in the source repository. Follow `Copied-From` to find them.

## Checking in CI

Run this on the main branch:

```sh
git fetch origin 'refs/notes/ai:refs/notes/ai'
prov check <base>..<tip>
```

The job needs the full history, not a shallow clone. It fails when:

- a commit on the main branch was made by an agent
- a merge brought in agent commits without the record that the gate writes
- a commit by a person contains AI-written lines that came in without a merge, a copy or an approval

## Configuration

`prov init` writes `.provenance.toml` in the repository root:

```toml
required_version = "=0.1.0"
agents = ["claude"]
logs_dir = "llm-logs"
protected = [".provenance.toml", ".claude/**", ".github/workflows/**"]
```

Everything else is a fixed convention:

| Convention | Value |
|---|---|
| Agent branches | `agent/<agent>/<date>-<name>` |
| Worktrees | `../<repo>.worktrees/` |
| Transcript archives | `llm-logs/<agent>/<session>-<hash>.tar.gz`, not tracked by git |
| Agent commit identity | `Claude <noreply@anthropic.com>` for Claude Code |

## Rules to know

- **One session in one worktree.** A second session in the same worktree stops commits there until one of them ends.
- **Agents do not work in your checkout of the main branch.** `prov` warns about a session there and records nothing for it.
- **Only merge commits.** Squash, rebase, cherry-pick and fast-forward of agent commits onto the main branch are not permitted.
- **Merge where the session ran.** Session records and transcripts are local to a clone, so the person whose clone ran a session also merges it.
- **Pulls fast-forward only.** `prov init` sets `pull.ff = only` for the repository.

## Limits

- The git hooks cannot stop an agent that wants to avoid them. The controls that count are the merge gate, which runs when a person merges, and `prov check` in CI.
- `prov` tells an agent commit from a person's commit by the author email. For a stronger control, require signed commits on the main branch in your forge.
- Transcript archives are files on your disk. `prov` records their hashes in git, but it does not back them up or publish them.

## License

MIT
