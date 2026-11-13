# Checking in CI

The merge gate runs on the machine of the person who merges. Local hooks can be skipped, so a second check runs on the human branch after the fact:

```sh
git fetch origin 'refs/notes/ai:refs/notes/ai'
prov check <base>..<tip>
```

The job needs:

- `prov` and git-ai, installed. The git-ai service is not needed: git-ai reads notes without it.
- The full history, not a shallow clone.
- `refs/notes/ai`, fetched.

It uses no local hooks and no live state, only what a clone holds.

## What it checks

First, `.provenance.toml` at `<tip>` must parse, and the binary must satisfy `required_version`. Then, for each commit on the first-parent line of the range, and on the first-parent line of each human branch that a merge in the range brings in:

| Commit | Check |
|---|---|
| Every commit | It is a human commit: its author email is not an agent email. |
| A merge that brings in agent commits | Each session has a trailer group. `Prov-Version` is present. Each doubtful range and each changed protected path has a `Prov-Approved` trailer. The config is read from the merge's first parent. |
| Any other commit | Its note names no AI session, except for a path with a `Copied-From` trailer or a `Prov-Approved: <path> ai` trailer. |

So the job fails when:

- a commit on the branch was made by an agent
- a merge brought in agent commits without the record that the gate writes, for example a forge merge
- a commit by a person contains AI-written lines that came in without a merge, a copy or an approval, for example a squash or cherry-pick of agent commits, or the work of an agent session in a human worktree

## Limits

- `prov check` cannot find a squash that has no note, such as one a forge made. It looks like a human commit. Protect the human branches on your forge so that agent branches cannot be merged there.
- It tells an agent commit from a person's commit by the author email. An agent that commits with a person's email passes. For a stronger control, require signed commits on the human branches in your forge; agent commits are never signed.

`prov` itself uses no hosted CI. It runs this check locally; see [Developing prov](development.md).
