# prov

`prov` keeps a record of what AI agents contribute to a git repository, and makes a human approve it before it reaches the main branch.

> **Status: version 0.1.0, not released.** Everything in this README is built and tested on Linux and macOS is not tested yet. The full rules are in the specification, [SPEC.md](SPEC.md).

## Background

I have been working on several research projects where the subject is (and depends on) AI-generated code, and I am very interested in accurately reporting and tracking the provenance of my contributions and an LLM's. I do not want to rely on soft attestations in the form of "self-disclosures" or git trailers. I want reviewers to be able to clearly see at the commit and line level where a contribution came from, as well as be able to trace changes to retained session logs.

I initially tried the `git` extension [git-ai](https://github.com/git-ai-project/git-ai) by itself but quickly noticed there are multiple situations where something could be misattributed (or not attributed at all), and I was abusing the `refs/notes/ai` trying to backfill these missing attributions. This ended up just not working out.

I then tried to get a handle on `git-ai` with my own custom `git` hooks, trailers, and `claude-code` plugins, and adopting a workflow where I restrict a session to a branch off my own branch, do not make changes in it myself, and merge it into my branch when I close out a session. This solved most of the issues, but since I was doing this in my research repository, and also in python, the history of the provenance workflow experiments was all mixed up with the research itself, the toolset was not portable to another future project, and could only handle one LLM session at a time. It also did not enforce any workflow explicitly and was still fairly brittle (data still occasionally got lost).

I decided this needed to be an independent CLI utility, and while not myself being fluent in rust it made the most sense for implementing this tool in with LLM code generation:

- `git-ai` itself is written in rust, so I can get compatibility for free and draw on it as a library
- it lends itself well to being a standalone CLI binary
- it seemed a reasonably safe choice for mainly LLM-generated code

Having done several failed proof-of-concepts I had settled on a final feature-set and scope. I spent hours iterating on a [spec](SPEC.md) in LLM review sessions, then had an LLM derive TLA+ specs from the final draft of the spec. I am even less qualified to review TLA+ specs, but I had concerns about the complexity of the state machine that was being produced, and wanted some additional insurance. I know enough at a high-level to instruct the LLM on my requirements and the premises that needed to be included and verified, and the TLA+ specs ultimately found numerous state bugs that were resolved. The [SPEC.md](SPEC.md) and associated `tla` files are included in the repository if anyone is interested in them.

I will check back in here if I ever get around to properly learning rust (or TLA+).

Status: PENDING

Anyway, what `prov` does is add an opinionated workflow, the various hooks and whatnot needed to run it, and the overall mechanism to ensure (as much as is feasible) that the correct information always flows into `git-ai`, ideally with the least amount of friction, and failing safe early and often with repair tools if something goes wrong before anything is committed. I also cover a gap where you may want to copy in a folder or directory from another repository with `git-ai`, or you know certain directories, files, and/or lines were LLM-generated, and you want to accurately report them. As-is, `git-ai` has no built-in mechanism for this.

De-Claude-ing the vile Claude-speak below is a work-in-progress. Everything below this point is un-edited Claude-talk.

### Shortcomings of git-ai out of the box

These were tested with git-ai 1.7.6. The evidence is in "Appendix: test evidence" in [SPEC.md](SPEC.md).

- **Changes outside a tool call are missed.** git-ai attributes a change through the checkpoints around it, and its Claude Code hooks place one before and one after each tool call. A file that a process writes after the call returns, for example a development server or a script left running, has no checkpoint. The agent's own hooks also record the model as `unknown`.
- **A line with no checkpoint gets a default author.** git-ai labels it by rules about the commit, not by what happened. Depending on who commits and what else is in the commit, the line goes to the committer as human, to an AI session that git-ai invents, or to no one.
- **Rewriting history can lose attribution.** git-ai moves a note to the new commit of an amend or a rebase only if its service sees the rewrite. Otherwise the new commit has no note, and the old note stays on the old commit. This happened to an amend in this repository.
- **Copied code loses its history.** When a file moves to another repository, its line attribution stays behind. The copy is credited to whoever commits it.
- **There is no way to correct a record.** git-ai has no command that fixes a record after the commit. Editing notes by hand mixes what git-ai observed with what someone decided later.
- **Some content is never attributed.** git-ai records no lines for binary files, lock files, vendored paths and the other paths on its ignore list. Attribution is per line: a one-character AI change makes the whole line AI, and there is no "mixed" category.
- **Failures are silent.** When git-ai's background service is down a checkpoint exits successfully and records nothing. In a repository that is not on git-ai's allow list git-ai skips every checkpoint. In both situations nothing prevents a commit.

### My own personal issues

- **Agent sessions started in the wrong place.** A session started outside the project's directory skipped the project's logging hooks, while git-ai's user-wide hooks still recorded its edits as AI. In that scenario the work was half documented.
- **Humans and agents shared one working tree.** Their edits mixed, so it was unclear who changed what, and agent commits were blocked.
- **The human made the agent's commits.** When an agent was blocked, the human ran its commit commands in their own shell, and the commits carried the human's identity.
- **The agent's identity was not one that git-ai recognizes.** With a custom commit identity, git-ai treated the agent's lines that had no checkpoint as human.
- **Hooks wrote tracked files.** Session logs that a hook wrote into tracked files were attributed inconsistently, and tidying them took extra commits.
- **One shared agent branch, one session at a time.** Merging while a session was still open, or ending a session before its log was archived, lost records. Parallel sessions were not possible.
- **The provenance tooling lived inside the project it documented.** Its own history polluted the record it was meant to keep, and it could not be reused elsewhere.

### What prov does about it

`prov` gives each agent session its own branch and worktree. It checkpoints every change in that worktree for the session that owns it, and marks a change that it cannot place as unattributed. Each agent commit names its session in a trailer, and the merge commit records the hash of each session's transcript. Agent work reaches a human branch only through a local merge, where a person must approve every line without a clear author. `prov` never edits git-ai's data: it sets authorship only through git-ai's public commands.

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
- git-ai 1.7.6 or later, with the repository in its `allow_repositories` setting. `prov` is tested with git-ai 1.7.6; `prov doctor` warns about a newer version until it is tested.
- Linux. macOS is supported by design but not tested yet.
- Claude Code as the agent. OpenCode is planned for a second phase.

## Installing

Build from source, with the Rust toolchain that `rust-toolchain.toml` names:

```sh
cargo install --path crates/prov --locked
```

Install it outside the repositories that use it: the git hooks run `prov` from your `PATH`.

Tab completion, with branch names and session IDs from the repository you are in, for bash, zsh and fish:

```sh
prov completions --install     # adds one line to your shell's startup file, once
```

`prov completions` alone prints the line, if you prefer to add it yourself. `prov init` does not change your startup file; it says when the line is missing.

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

This creates the branch `agent/claude/<date>-parser-tests`, a worktree for it in `../<repo>.worktrees/claude-<date>-parser-tests`, and starts Claude Code there. `--path <dir>` puts the worktree in another directory; its name still comes from the branch. The agent works and commits as usual. When you end the session, `prov` archives its transcript.

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

Remove the worktree when you are done. The branch stays as a record. `prov finish` prints the command that deletes the branch: `git branch -d` when the branch is merged into the branch you have checked out, `git branch -D` with a warning when it is not.

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
| `prov doctor` | Checks that the setup is intact, reports sessions that need attention, and finds commits that git-ai missed (no note). |
| `prov start <agent>` | Creates a worktree on a new agent branch and starts the agent there. |
| `prov check` | Lists agent branches that are not merged yet. |
| `prov check <agent branch>` | Shows what the merge gate will ask about, before you merge. |
| `prov check <base>..<tip>` | Checks commits already on the main branch, and on the feature branches merged into it. This is the command for CI. |
| `prov approve <path>...` | Records your decision about a file. See "Approving" below. |
| `prov session list` / `show` / `close` / `drop` | Shows sessions, ends one that did not end by itself, or deletes the record of one. |
| `prov finish <branch>` | Removes an agent worktree and keeps the branch. |
| `prov cp` | Copies files from another repository, with an author for every line. |
| `prov completions [--install]` | Prints the line that turns on tab completion, or adds it to your shell's startup file. |
| `prov trace export` | Writes the authorship record in the Agent Trace format. Optional. |

Every command that changes something accepts `--dry-run`. Every command that shows something accepts `--json`.

## Approving

`prov approve` has three uses. Only a person can run it; it refuses inside an agent session.

| Option&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp; | When you use it | Effect |
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

Other ways to give the authors:

```sh
prov cp --from ../other-repo src/ src/ --plan plan.toml   # write the plan to a file and stop
prov cp --apply plan.toml                                 # apply a plan file
```

Then you commit. The commit gets a `Copied-From` trailer for each file, which names the source repository, commit and path. If the path in your repository is different, ` => <new path>` follows.

- Commit a copied file as it is. `prov` refuses a commit in which a copied file is changed; change it in a later commit.
- After the commit, `prov` checks in the background that git-ai recorded the authors of the plan. `prov doctor` reports a file that failed.
- git-ai keeps no line authorship for some paths, for example `vendor/` and lock files. A copy to such a path has the trailer only, and `prov doctor` shows a note.

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
known_gaps = []      # commits with no git-ai note that you accept; prov doctor then lists them as notes
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
- **Stop background processes when a session ends.** A process that keeps writing in the worktree, such as a development server, is not swept, and the next session there would take its changes as its own. `prov session close`, `prov finish` and `prov start --resume` list the processes that still run in the worktree.

## Limits

- The git hooks cannot stop an agent that wants to avoid them. The controls that count are the merge gate, which runs when a person merges, and `prov check` in CI.
- `prov` tells an agent commit from a person's commit by the author email. For a stronger control, require signed commits on the main branch in your forge.
- Transcript archives are files on your disk. `prov` records their hashes in git, but it does not back them up or publish them.
- Every file in a transcript archive has the date 1970-01-01. This is on purpose: the same transcript always gives the same archive and the same hash. The times of the messages are in the transcript itself.
- `prov` waits for the agent process to exit before it archives a transcript. If it does not know the process ID, it archives at once, and the archive can miss the agent's last work. `prov session close` and `prov finish` warn when that is the case.
- `prov` depends on how git-ai attributes lines. `prov doctor` warns when git-ai is newer than the version that `prov` is tested with.

## Developing prov

The checks run locally in Podman. There is no hosted CI.

```sh
ci/dev.sh test --workspace --locked     # day to day: tests with a cached build
ci/dev.sh --models                      # the TLA+ models in tla/
ci/run.sh                               # every check, on a clean clone of HEAD
cp ci/pre-push "$(git rev-parse --git-path hooks)/pre-push"   # run ci/run.sh before each push
```

The container pins git 2.53, git-ai 1.7.6, the Rust toolchain and the TLA+ tools. In it, an integration test that cannot find git-ai fails instead of skipping. The git-ai contract tests (`crates/prov/tests/gitai_contract.rs`) check the git-ai behavior that `prov` depends on. Run them against a new git-ai version before you raise the tested version (`prov_gitai::TESTED_VERSION`).

## License

MIT
