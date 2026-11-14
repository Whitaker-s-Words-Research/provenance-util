# prov

`prov` keeps a record of what AI agents contribute to a git repository, and makes a human approve it before it reaches the main branch.

> **Status: version 0.1.0, not released.** Everything in this README is built and tested on Linux and macOS is not tested yet.

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

These were tested with git-ai 1.7.6.

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

`prov` answers them with plain git and [git-ai](https://usegitai.com): git-ai's notes hold line authorship, commit trailers name the session and the approvals, and the merge commit holds the hash of each session's transcript. The record is in your git history, so anyone can read it with `git log`. See [The record](docs/record.md).

## How it works

1. **Each agent session gets its own worktree and branch.** Agents commit only on `agent/...` branches. Sessions never share files, so several can run at once.
2. **A person merges the agent branch.** The merge is the review. When you run `git merge`, the merge gate checks the incoming commits.
3. **The gate stops on anything doubtful.** A line is doubtful when git-ai cannot say who wrote it. A change to a protected file, such as the agent's own settings, is also doubtful. You approve each item, or you stop the merge.
4. **The merge commit carries the record.** It names each session and the hash of its transcript.

A human owns the responsibility for what goes into the main branch. The same rule applies to every branch that is not an agent branch (`agent/**`). For that reason, `prov` supports only local merges, never the merge button of a forge such as GitHub. Details: [Workflow](docs/workflow.md) and [How it works](docs/how-it-works.md).

On a shared repository, protecting the human branches on your forge is strongly recommended: require signed commits, and permit only merge commits. Reason: `prov` tells a person's commit from an agent's by the author email, so an agent that uses a person's email passes. Agent commits are never signed. A squash or rebase merge on the forge drops the record. See [Checking in CI](docs/ci.md#limits).

## Quick start

Requires git 2.31+, git-ai 1.7.6+ with the repository in its `allow_repositories`, Linux, and Claude Code. See [Getting started](docs/getting-started.md) for the full requirements and what each step does.

```sh
cargo install --path crates/prov --locked          # install outside the repositories that use it
prov init --agent claude                           # once in each clone
git add .provenance.toml .claude/settings.json && git commit -m "Set up prov"
prov doctor

prov start claude --name parser-tests              # agent works in its own worktree
prov check agent/claude/20261111-parser-tests      # what the gate will ask about
git merge agent/claude/20261111-parser-tests       # the gate runs here
prov approve --merge notes.txt && git commit       # if the gate stopped
prov finish agent/claude/20261111-parser-tests     # remove the worktree, keep the branch
```

## Documentation

The guides are in [docs/](docs/README.md):

- [Getting started](docs/getting-started.md): requirements, installing, tab completion, setup and a first session.
- [Workflow](docs/workflow.md): sessions, the merge gate, approvals, what is not permitted, and the rules to know.
- [Commands](docs/commands.md): every command and its options.
- [Copying files](docs/copying.md): `prov cp`, which copies files from another repository with an author for every line.
- [Checking in CI](docs/ci.md): `prov check <base>..<tip>` on the main branch.
- [Configuration](docs/configuration.md): `.provenance.toml`, the fixed conventions, and what `prov init` changes.
- [The record](docs/record.md): trailers, transcript archives and Agent Trace export.
- [How it works](docs/how-it-works.md): hooks, the sweep, taint and session states.
- [Troubleshooting](docs/troubleshooting.md): `prov doctor`, common problems, and the limits of `prov`.
- [Developing prov](docs/development.md): building and testing in Podman.

The full rules, with their reasons, are in [SPEC.md](SPEC.md).

## License

MIT
