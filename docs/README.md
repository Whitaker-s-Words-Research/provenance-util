# prov documentation

These pages explain how to use `prov`. The full rules, with their reasons and the test evidence, are in the specification, [SPEC.md](../SPEC.md). When a page here and the specification disagree, the specification is right.

| Page | Read it to |
|---|---|
| [Getting started](getting-started.md) | Install `prov`, set up a repository, and run and merge a first agent session. |
| [Workflow](workflow.md) | Understand sessions, worktrees, the merge gate and approvals in daily use. |
| [Commands](commands.md) | Look up a command and its options. |
| [Copying files](copying.md) | Copy files from another repository with an author for every line (`prov cp`). |
| [Checking in CI](ci.md) | Check a human branch after the fact with `prov check <base>..<tip>`. |
| [Configuration](configuration.md) | Edit `.provenance.toml`, and see the fixed conventions and the git settings that `prov init` makes. |
| [The record](record.md) | Read what `prov` and git-ai leave in the history: trailers, notes, transcript archives, Agent Trace export. |
| [How it works](how-it-works.md) | See what the hooks do: the sweep, taint, the session states, the archive step. |
| [Troubleshooting](troubleshooting.md) | Fix a problem that `prov doctor` reports, and know the limits of `prov`. |
| [Developing prov](development.md) | Build and test `prov`, and work on the TLA+ models. |

Status: version 0.1.0, not released. Tested on Linux only.
