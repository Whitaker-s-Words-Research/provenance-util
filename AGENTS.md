# Instructions for AI agents

`prov` is a Rust command-line tool that records what AI agents contribute to a git repository, through git-ai, and makes a human approve agent work before it reaches a human branch. Read these first:

- [SPEC.md](SPEC.md): the rules, with their reasons and the test evidence. It is the authority. When code and spec disagree, ask which one is wrong.
- [PLAN.md](PLAN.md): the current status, what is not tested yet, and the open decisions.
- [docs/](docs/README.md): the user guides. Keep them true when behavior changes.

## Constraints from the owner

- **Design choices go to the owner as questions**, with a recommendation. Do not change a rule in SPEC.md on your own.
- **The owner commits and signs.** Leave changes uncommitted unless asked. Never push without asking.
- **No API costs outside Claude Code.** Before a test calls a model, say what it bills to. Ask before any `claude -p` run.
- **No hosted CI.** All checks run locally in Podman.
- **A human owns what goes into a human branch.** Never add support for a forge's merge button.
- **`prov` never writes git-ai data.** No writes to `refs/notes/ai`, no `git notes --ref=ai` edits, no changes to git-ai's working logs. Authorship is set only through public git-ai checkpoint commands, and read only through git-ai's JSON output.

## Working in this repository

This repository uses `prov` on itself.

- Work only in an agent worktree, on a branch that matches `agent/**`. `prov` refuses an agent commit on any other branch. If your session started in a human worktree (for example on `main`), stop and tell the human.
- Do not edit `.provenance.toml`, `.claude/**`, `CLAUDE.md` or `AGENTS.md` unless the human asks. They are protected paths, and every change needs a human approval at the merge gate.
- Stop any background process you started (a server, a watcher) before the session ends. `prov` does not sweep changes made after the session.

## Building and testing

Build and test in the container, not on the host. The checkout is on a slow 9p mount.

```sh
ci/dev.sh test --workspace --locked                            # all tests, cached build, ~30 s
ci/dev.sh test -p prov --test <file> <filter>                  # one test
ci/dev.sh clippy --workspace --all-targets --locked -- -D warnings
ci/dev.sh --models                                             # the TLA+ models in tla/
ci/run.sh                                                      # the clean full check of HEAD
```

- Clippy runs with `pedantic` and `-D warnings`. Do not use `unwrap` or `expect` outside tests: a panic in a hook is the worst way to fail.
- Run on the host only the tests that must run on the slow mount: `PROV_TEST_REPOS="$PWD/target/p9" cargo test --workspace --locked` (about 15 minutes).
- Keep hooks fast. Each git process costs 25 to 100 ms on 9p. Do not add git processes or walks over the worktree to a hook.

## Testing git-ai behavior

- Your own session's git-ai hooks attribute changes in any repository you touch to your session. A test of attribution must use its own home directory and git-ai service, as `crates/prov/tests/common/mod.rs` does. For an experiment by hand: `env -i HOME=<short dir in /tmp> PATH=...` and its own `git-ai bg start`. The home must be a short path (Unix socket length limit).
- git-ai skips every checkpoint, silently, in a repository that is not in `allow_repositories`. It matches remote URLs only.
- A note arrives 1 to 6 seconds after a commit. Run `git-ai await --timeout 60` before you read it.
- `crates/prov/tests/gitai_contract.rs` checks each git-ai behavior that a rule depends on. Run it against a new git-ai version before raising `prov_gitai::TESTED_VERSION`.

## Making a design change

1. Write the rule in SPEC.md, and model it in TLA+ (`tla/`) if it depends on the order of events. Each model lists its premises about git, git-ai or Claude Code.
2. Test each premise with the real tools before the decision goes to the owner, and record the result in "Appendix: test evidence" in SPEC.md. If a premise cannot be tested, say so in the question.
3. When a premise turns out false, find every rule that rests on it and correct them in the same change.

A model checks the order of events. It cannot check a premise. See [tla/README.md](tla/README.md) before you change a model: invariants change only with the owner's review, and a `bug-*` config that starts to pass means the invariant became too weak.

## Commits

- Conventional Commits: `feat(prov-core): ...`, `fix(prov): ...`, `docs: ...`, `chore: ...`. Do not copy older subjects such as `Plan: ...`.
- Agent commits use the identity `Claude <noreply@anthropic.com>` and are not signed. `.claude/settings.json` sets both; do not override them.

## User-facing text

- Put every text that prov shows to a person or an agent in `crates/prov-messages`, in the module of its area. A fixed text is a `const`, and a text with values is a function. The code adds no words to a text. See "User-facing text" in [docs/development.md](docs/development.md).
- Help texts are the doc comments in `crates/prov/src/main.rs`. After a change, write `crates/prov/tests/snapshots/help.txt` again: `PROV_UPDATE_SNAPSHOTS=1 ci/dev.sh test -p prov --test help`.
- Not in `prov-messages`: the JSON output, trailer names, and the files that `prov init` writes. A change to them is a change of format.

## Writing style

SPEC.md, the docs and the help texts use plain, short sentences: one fact per sentence, active voice, "Reason:" for the why. Follow that style, and keep a term the same everywhere (human branch, agent worktree, doubtful line, integration).
