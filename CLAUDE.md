# Claude Code instructions

@AGENTS.md

## Specific to Claude Code

- Start sessions with `prov start claude` from the human checkout, or resume with `prov start claude --resume <session ID>`. A session resumed with plain `claude --resume` in another directory is not bound to its worktree: its commits get no `Prov-Session` trailer and no sweep.
- If the SessionStart hook prints a `prov` warning (a human worktree, or a contended worktree), stop and tell the human. Do not work around it.
- A commit runs `git-ai await` in `pre-commit`, which can take up to 60 seconds. Do not interrupt it. A "did not finish in 60 seconds" warning is not a failure: check the note afterwards with `git-ai stats HEAD --json`.
- The global git-ai hooks checkpoint every tool call, including Bash. Every file that a Bash command changes, for example through a formatter or a generator, is attributed to this session.
- Do not use `git commit --no-verify`. It skips the sweep, and the lines then go to a session that git-ai invents, which the gate reports as doubtful.
- Do not run `git rebase` on commits with notes: git-ai 1.7.6 drops the notes. An amend keeps them while the git-ai service runs.
