# Getting started

## Requirements

- git 2.31 or later.
- git-ai 1.7.6 or later. `prov` is tested with git-ai 1.7.6; `prov doctor` warns about a newer version until it is tested.
- git-ai must allow the repository. By default `allow_repositories` is empty, and git-ai allows every repository, also one with no remote. If you set `allow_repositories`, git-ai matches it against remote URLs only: a repository with no remote is then never allowed, not even by `*`. Give it a remote whose URL matches; the remote need not exist yet. In a repository that is not allowed, git-ai ignores every checkpoint.
- The git-ai service must run (`git-ai bg start`). Without it, a checkpoint records nothing, and `prov` refuses commits.
- Optional: git-lfs, only to copy files that a source repository stores with Git LFS (`prov cp`). Without it, such a copy stops with an error. If you use git-lfs in a repository with `prov`, run `git lfs install --local --force` and then `prov init` again: `prov init` keeps LFS's hooks and runs them after its own.
- Linux. macOS is supported by design but not tested yet.
- Claude Code as the agent. OpenCode is planned for a second phase.
- Strongly recommended on a shared repository: branch protection for the human branches on your forge, with signed commits required and merge commits only. Reason: without signed commits, an agent that commits with a person's email passes as that person. A squash or rebase merge on the forge drops the record. See [Checking in CI](ci.md#limits).

## Installing

Build from source, with the Rust toolchain that `rust-toolchain.toml` names:

```sh
cargo install --path crates/prov --locked
```

Install it outside the repositories that use it: the git hooks run `prov` from your `PATH`. There is no release yet.

Tab completion, with branch names and session IDs from the repository you are in, for bash, zsh and fish:

```sh
prov completions --install     # adds one line to your shell's startup file, once
```

`prov completions` alone prints the line, if you prefer to add it yourself. `prov init` does not change your startup file; it says when the line is missing.

## Setting up a repository

```sh
prov init --agent claude
git add .provenance.toml .claude/settings.json
git commit -m "Set up prov"
prov doctor
```

`prov init` writes:

- `.provenance.toml`, the config. See [Configuration](configuration.md).
- `.claude/settings.json`: the Claude Code hooks, the agent's commit identity, and `commit.gpgsign=false` for the agent's commands.
- One-line hook shims in git's hooks directory (`pre-commit`, `prepare-commit-msg`, `post-commit`, `post-merge`). An existing hook is kept as `<name>.local` and still runs.
- The git options `merge.ff = false` and `pull.ff = only`.
- `logs_dir` in `.gitignore`, if it is inside the repository.

The shims and git options are not tracked. **In every new clone, run `prov init` again.** It leaves the tracked files as they are.

`prov doctor` checks the setup, git-ai and its service, and the sessions. Run it whenever something looks wrong.

## A first session

Start an agent in its own worktree:

```sh
prov start claude --name parser-tests
```

This creates the branch `agent/claude/<date>-parser-tests`, a worktree for it in `../<repo>.worktrees/claude-<date>-parser-tests`, and starts Claude Code there. The agent works and commits as usual. When you end the session, `prov` archives its transcript.

See what waits, then merge it from your own checkout of the main branch:

```sh
prov check                                        # agent branches not merged yet
prov check agent/claude/20261111-parser-tests     # what the gate will ask about
git merge agent/claude/20261111-parser-tests
```

If the merge gate finds doubtful items, it stops and lists them. The merge stays in progress:

```sh
prov approve --merge notes.txt     # shows the item and asks for a typed "yes"
git commit                         # finishes the merge; the gate runs again
```

To give up instead, run `git merge --abort`.

Remove the worktree when you are done. The branch stays as a record:

```sh
prov finish agent/claude/20261111-parser-tests
```

`prov finish` prints the command that deletes the branch, and does not run it: `git branch -d` when the branch is merged into the branch you have checked out, `git branch -D` with a warning when it is not.

To throw away a branch whose work did not work out, use `prov discard <branch>` instead. It lists what it deletes, and deletes it with `--yes`. See [Discarding](workflow.md#discarding).

## Next

- [Workflow](workflow.md): what each step does, and what to do when it stops.
- [The record](record.md): what the merge commit and the notes look like afterwards.
