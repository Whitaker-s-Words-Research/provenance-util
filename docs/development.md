# Developing prov

The current status and open work are in [PLAN.md](../PLAN.md). The specification is [SPEC.md](../SPEC.md); change it before the code.

## Layout

| Path | Contents |
|---|---|
| `crates/prov-messages` | every text that prov shows to a person or an agent; see [User-facing text](#user-facing-text) |
| `crates/prov-core` | config, live state, session state machine, lock, trailer format |
| `crates/prov-git` | repository and worktree discovery, git commands, hook handling |
| `crates/prov-adapters` | the adapter interface and the Claude Code adapter |
| `crates/prov-gitai` | git-ai commands and their JSON output, copy plan and replay |
| `crates/prov` | the command line: init, doctor, check, start, gate, sweep, archive and the other commands |
| `crates/prov/tests` | integration tests, with recorded Claude Code hook payloads in `fixtures/` |
| `vendor/agent-trace` | the pinned Agent Trace 0.1.0 schema |
| `tla/` | the TLA+ models; see [`tla/README.md`](../tla/README.md) |
| `ci/` | the container image and scripts for the local checks |

## User-facing text

All text that prov shows is in two places. Review a change of wording there.

- `crates/prov-messages/src/`: output, warnings, errors, and the messages that hooks give an agent. One module for each area, named after the source module that uses it. A fixed text is a `const`, and a text with values is a function.
- `crates/prov/tests/snapshots/help.txt`: the help texts of all commands. They come from the doc comments in `crates/prov/src/main.rs`. The test `help` fails when they change. To write the file again: `PROV_UPDATE_SNAPSHOTS=1 ci/dev.sh test -p prov --test help`. `ci/dev.sh` copies the file from the container back to the checkout.

The code adds no words to a text. Reason: a reviewer reads each sentence in one place.

Not in `prov-messages`: the JSON output, trailer names, and the content of the files that `prov init` writes. Programs read them, and a change to them is a change of format.

## Running the checks

The checks run locally in Podman. There is no hosted CI.

```sh
ci/dev.sh test --workspace --locked     # day to day: tests with a cached build
ci/dev.sh clippy --workspace --all-targets -- -D warnings
ci/dev.sh --models                      # the TLA+ models in tla/
ci/dev.sh --clean                       # remove the build cache
ci/run.sh                               # every check, on a clean clone of HEAD
ci/run.sh --worktree                    # every check, on the working tree as it is
cp ci/pre-push "$(git rev-parse --git-path hooks)/pre-push"   # run ci/run.sh before each push
```

- `ci/dev.sh <cargo arguments>` runs any cargo command in the check image, on the working tree, with the build cached in a Podman volume. The full suite takes about 30 seconds.
- The image (`ci/Containerfile`) pins Ubuntu 26.04 with git 2.53, git-ai 1.7.6, the Rust toolchain of `rust-toolchain.toml` and tla2tools 1.7.4, each by checksum.
- The containers run with `--init`. Without it, a stopped git-ai service stays as a zombie and tests that wait for it fail.
- In the container, `PROV_TEST_REQUIRE_GITAI=1` is set, so an integration test that cannot find git-ai fails instead of skipping.
- Builds use `--locked`.
- After each merge of an agent branch, run `ci/run.sh` on the human branch before the next merge. Reason: two branches can each pass their checks and fail together. This happened when `prov revise` and `prov discard` each wrote the help snapshot. To run only the tests, add `--no-fail-fast`: a plain `cargo test` stops at the first test binary that fails and hides later failures.

## Tests

- **Integration tests** use temporary repositories with worktrees and recorded hook payloads.
- **Each test gets its own home directory and git-ai service** (`crates/prov/tests/common/mod.rs`) and runs commands with a clean environment. Otherwise the git-ai hooks of the agent session that runs the tests would attribute the test's changes to that session. The home must be a short path, because of the Unix socket length limit. For an experiment by hand, do the same: `env -i HOME=<short dir> PATH=...` and its own `git-ai bg start`.
- **git-ai contract tests** (`crates/prov/tests/gitai_contract.rs`) check each git-ai behavior that a rule depends on. Run them against a new git-ai version before you raise the tested version (`prov_gitai::TESTED_VERSION`).
- **A latency test** fails if an agent hook takes more than 100 ms at the 95th percentile on a local disk.
- **Slow file systems:** `PROV_TEST_REPOS=<dir>` puts the test repositories on another file system, for example a 9p mount. The suite must pass there too; on 9p it takes about 15 minutes. Keep `prov` free of extra git processes and walks over the worktree: each git process costs 25 to 100 ms there.

### git-ai timing

- A note arrives 1 to 6 seconds after a commit. Run `git-ai await --timeout 60` before you read it.
- A checkpoint is applied after the command returns. `prov` runs `git-ai await` after its checkpoints in `pre-commit` and `prov approve`.
- `git-ai bg start` does nothing while the old service is still exiting.

## TLA+ models

The rules that depend on the order of events have TLA+ models in `tla/`. `ci/dev.sh --models` runs `tla/check-models.sh`: each `<Module>.cfg` must pass, and each `bug-*`, `limit-*` and `reach-*` config must fail with an invariant violation. See [`tla/README.md`](../tla/README.md) for the naming and for how to change a model.

## How a design decision is made

1. Write the rule in SPEC.md, and model it in TLA+ if it depends on ordering. Each model lists its premises: what it takes as a fact about git, git-ai or Claude Code.
2. Test each premise with the real tools before the decision is made. Record the result in "Appendix: test evidence" in SPEC.md.
3. When a premise turns out false, find every rule that rests on it and correct them in the same change.

## Tested versions

git 2.53, git-ai 1.7.6, Claude Code 2.1.322, Rust 1.100.0. On macOS, run `cargo test --workspace --locked` on a Mac before a release.

## Releases

There is no release process yet. Build from source. Binaries will be published in GitHub releases; the crates are not published on crates.io.
