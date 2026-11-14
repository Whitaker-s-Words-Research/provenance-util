# TLA+ models for prov

These models check the rules in `../SPEC.md` that depend on ordering. SPEC.md gives each rule with its reason, and is the authority. When a model and the prose disagree, fix the prose first, then the model, then the code.

| Module | Checks | Rule IDs |
|---|---|---|
| `ProvSweep.tla` | Sweep attribution in one agent worktree: occupants, taint, contention, the sweep at SessionEnd | R-SWEEP-1 to R-SWEEP-5 |
| `ProvSessions.tla` | Session states and the detached archive process | R-SESS-1 to R-SESS-3 |
| `ProvIntegration.tla` | Gate, then editor, then commit, then post-commit cleanup | R-INT-1, R-INT-2 |
| `ProvDiscard.tla` | `prov discard` of a branch while its sessions resume and a human integrates it | R-DISC-1 |

Each model has switch constants that turn a rule off. With a switch off, the model behaves as SPEC.md 0.1 said, or as the prov 0.1 code did (`ProvSessions.bug-state-only`).

One liveness property is checked: `ClosingEnds` in `ProvSessions`, with `LiveSpec` (the safety spec plus weak fairness of the archive process; the 5-minute rule is the calculated state `EffSt`). `ProvSessions.live.cfg` must pass. `ProvSessions.reach-stuck-closing.cfg` checks the same property without fairness and must fail (TLC exit code 13), so the property is not trivial.

## Config naming

| Pattern | Expected result |
|---|---|
| `<Module>.cfg` | passes |
| `<Module>.<other name>.cfg` | passes: the rules hold under another assumption (`ProvSessions.live`, the liveness property) |
| `<Module>.bug-*.cfg` | invariant violated: the old rule's bug (regression canary) |
| `<Module>.limit-*.cfg` | invariant violated: an accepted limit that SPEC.md documents |
| `<Module>.reach-*.cfg` | invariant violated: proves that an interesting state is reachable, so a pass is not trivial |

## Running

```bash
TLA2TOOLS=/path/to/tla2tools.jar ./check-models.sh      # needs Java 11+
VERBOSE=1 TLA2TOOLS=... ./check-models.sh               # print TLC output on unexpected results
```

Checked with tla2tools 1.7.4 (TLC 2.19, SHA-256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`) on Temurin 17. A full run takes about 30 seconds.

## Premises

Each module starts with a list of premises: what it takes as a fact about git, git-ai or Claude Code. A model checks the order of events. It cannot check a premise. Each premise names a row of "Appendix: test evidence" in `../SPEC.md`, or says "untested". Test a premise with the real tools before a decision that rests on it goes to the owner. A false premise was found once: the first taint design assumed that a change with no checkpoint becomes a doubtful line, and git-ai gives it to an AI session of the commit.

## Changing a model

- When you add or change a premise, add or name its evidence row in SPEC.md in the same change.
- Edit actions to follow the SPEC.md text. Invariants, the `*Sound`, `*Current` and `NoNeeded*` definitions, change only with the owner's review.
- When a fix makes a `bug-*` config pass, the invariant has become too weak. Don't delete the canary.
