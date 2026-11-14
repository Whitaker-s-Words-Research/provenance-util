# prov: status and open work

The specification is [SPEC.md](SPEC.md); read it first. It has the rules with their reasons and the test evidence. The user-facing overview is [README.md](README.md), and the user guides are in [docs/](docs/README.md). The history of the work is in git.

## Status

- Phase 1 is done and merged into `main`: the core (sessions, hooks, sweep, merge gate, `prov check`), the commands, `prov cp`, `prov discard` (R-DISC-1), `prov revise` (R-REV-1), and the rules from the TLA+ model review.
- The review of 2026-11-13 is worked through on the branch `agent/claude/20261113-6631`, not merged yet: whole-file items for binary files, mode changes and deleted files; the gate refuses an agent; merge commits only for human branches on a forge; `prov revise` counts insertions, later passes and several merges; `prov session close` is renamed to `prov session archive`.
- Version 0.1.0, not released. There is no release process yet (SPEC, "Build and release").

- Checks: 117 tests (`ci/dev.sh test --workspace --locked`; on 2026-11-13 also on the 9p mount with `PROV_TEST_REPOS`, all passed), 29 TLA+ model configs (`ci/dev.sh --models`), clippy with `-D warnings`.

- OpenCode is phase 2, in "Appendix: phase 2, OpenCode" in the spec. Phase 1 rules must stay general; the appendix lists the six that must.

## Not tested yet

- `prov discard` racing a real merge or a resume. The model `ProvDiscard` checks the order; the tests run each refusal one at a time.
- Contended worktrees with two real Claude Code processes. Only recorded hook payloads were tested.
- A real session resumed between the gate and `post-commit`, or while the merge editor is open. Premise P4 of `tla/ProvIntegration.tla` is reasoned, not tested.
- macOS, entirely. This includes the process scan with `lsof` (the warning of R-SWEEP-4).
- Killing a real archive process during its write. A unit test covers what such a kill leaves (R-ARCH-1).
- `prov cp`: an editor that needs the terminal (the test editor is a script), a source with Git LFS or a clean filter, a copy of thousands of files.
- The file lock between a host and a VM on a shared mount. Tested only inside one machine, also on 9p.
- Why git-ai's own Claude Code hooks recorded the model in one live run and `unknown` in another. The `Prov-Model` trailer covers it either way. No issue will be filed with git-ai (owner's decision).

## Open decisions

Each goes to the owner as a question with a recommendation.

- Should `prov check <base>..<tip>` warn about a commit on the first-parent line that adds lines and has no git-ai note? It would find a squash made on a forge, which passes today (test `forge_merge_methods_of_a_human_branch`). It would also flag the commits of a person who does not use git-ai. `prov doctor` already reports such commits on local branches.
- `prov revisions`, a report of the revisions of each integration, is out of scope for now (owner, 2026-11-13).
- A config key for the worktrees directory, so that agent worktrees can live on a local disk when the repository is on a slow mount.
- Choices from session 2 that the spec does not fix, never confirmed by the owner:
    - `prov approve` reads the typed `yes` from `/dev/tty`. With no terminal it refuses. `--dry-run` shows the items with no prompt.
    - `prov session archive` and `prov finish --override` run the archive step in the foreground, not in a detached process.
    - `prov init` keeps an existing `.provenance.toml` that parses. It replaces the file only with `--force`, and only when `--agent` asks for other agents or the file does not parse. It replaces a differing value in `.claude/settings.json` only with `--force`.
    - `prov doctor` decides "git-ai allows the repository" from `git-ai config allow_repositories` and `exclude_repositories`, matched against the remote URLs. This repeats git-ai's rule; git-ai has no command that answers the question.
    - `prov trace export` finds the commits with a note through `git notes --ref=ai list`. The content comes from `git-ai diff --json`.
    - The content hash of a `--human` or `--ai` approval is the git blob ID of the file.
    - `prov start` is not refused for an agent. The spec says "run by a human" and gives no refusal.
    - The JSON Schema validator is written in-house (`crates/prov/src/schema.rs`), for the keywords that the pinned schema uses.
- `prov discard` writes its texts in `crates/prov/src/discard.rs`, not in `prov-messages` (AGENTS.md, "User-facing text"). A plain fix.
- Choices from session 3 made without asking:
    - How `prov cp` reads `<dest>`: one file, one directory, several sources.
    - A copy record with the result `moved` or `ignored` stays until someone deletes it.
- Items from an earlier spec review, known only by name. Ask the owner for the review's text:
    - The incoming range when an agent branch merged a different human branch.
    - `post-merge` and the first-parent line.
    - `MERGE_MSG` and the rule near SPEC line 63 (`prov` writes inside `.git` only in `.git/provenance/` and the hook shims).
    - git-ai `allow_repositories`.
    - Untested environment variables.
    - `prov approve` without a terminal.
- The release process.

## How a design decision is made

1. **Write the rule in the spec, and model it in TLA+ if it depends on ordering.** Each model lists its premises: what it takes as a fact about git, git-ai or Claude Code. Each premise names a row of "Appendix: test evidence" in the spec, or says "untested".
2. **Test each premise with the real tools before the decision goes to the owner.** If a premise cannot be tested yet, the question to the owner says so.
3. **When a premise turns out false, search the spec for every other rule that rests on it**, and correct them in the same change.

A model checks the order of events. It cannot check a premise.

## Working in this repository

- **Build and test in the container, not on the host.** The checkout is on a slow 9p mount. `ci/dev.sh <cargo arguments>` runs any cargo command in the image of the local checks, on the working tree as it is, with the build cached in a Podman volume (the full suite takes about 30 seconds). `ci/dev.sh --models` checks the TLA+ models; `ci/dev.sh --clean` removes the cache. `ci/run.sh [<commit> | --worktree]` is the clean full check. `ci/pre-push` runs it for each pushed commit; install it in each clone: `cp ci/pre-push "$(git rev-parse --git-path hooks)/pre-push"`.
- **The containers run with `--init`.** Without it, a stopped git-ai service stays as a zombie and a test that waits for it to exit fails.
- **The tests set `PROV_TEST_REQUIRE_GITAI=1` in the container**, so a missing git-ai fails instead of skipping.
- **Tests of `prov` on a slow file system run on the host:** `PROV_TEST_REPOS="$PWD/target/p9" cargo test --workspace --locked`. It takes about 15 minutes; the latency test uses a limit relative to the cost of git there. Keep `prov` free of extra git processes and of walks over the worktree: each git process costs 25 to 100 ms on 9p, and each file that git examines about 2 ms.
- **The harness gives each test its own home and git-ai service** (`crates/prov/tests/common/mod.rs`). Test repositories need no remote, and the agent's own git-ai hooks cannot reach them. The home must be a short path, because of the Unix socket length limit. For an experiment by hand, do the same: `env -i HOME=<short dir> PATH=...` and its own `git-ai bg start`.
- **git-ai timing.** A note comes 1 to 6 seconds after a commit: run `git-ai await --timeout 60` before reading it. A checkpoint is applied after the command returns, so `prov` runs `git-ai await` after its checkpoints in `pre-commit` and `prov approve`. `git-ai bg start` does nothing while the old service still exits; `Env::stop_service` waits for it.
- **With `merge.ff = false`, git refuses `git merge --squash`.** A squash needs `--squash --ff`.
- **Tested versions:** git 2.53, git-ai 1.7.6, Claude Code 2.1.322 (2.1.325 for the agent environment in merge hooks), Rust 1.100.0. The container image pins Ubuntu 26.04 (git 2.53), git-ai 1.7.6 and tla2tools 1.7.4, each by checksum. Before raising the git-ai version, run `cargo test --test gitai_contract` against it, then raise `prov_gitai::TESTED_VERSION`.
- **Measure time with the binary on a local disk.** Starting a binary from the 9p mount costs about 40 ms.

## Constraints from the owner

- **No API costs outside Claude Code.** Before a test calls a model, say what it bills to. Short `claude -p` runs on the Claude Code login are acceptable in small numbers; ask first.
- **Design choices go to the owner as questions**, with a recommendation, not as direct edits to the spec.
- **A human owns what goes into a human branch.** Integration through a forge's merge button is never supported.
- **The owner commits and signs.** Leave changes uncommitted unless asked, and ask before any push.
- **No hosted CI.** Checks run locally in Podman.
