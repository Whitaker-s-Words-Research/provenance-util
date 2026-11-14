# Troubleshooting

Start with:

```sh
prov doctor          # add --deep to also run git-ai debug
```

It checks:

- `.provenance.toml` parses, and the binary satisfies `required_version`.
- The hooks directory is outside every worktree, and each shim has the expected content.
- `merge.ff` is `false` and `pull.ff` is `only`.
- `.claude/settings.json` sets the agent identity and `commit.gpgsign=false`.
- `.provenance.toml` and the agent settings are committed.
- git-ai is installed, its version is 1.7.6 or later (a newer, untested version is a warning), its service runs, and it allows the repository.
- git-ai's integration for each agent is installed.
- No session is stale or Failed, no worktree is contended, no copy check failed.
- No session record is left over after its branch was merged.
- Every commit on a local branch that adds lines has a git-ai note.

## Common problems

### A commit is refused: the git-ai service is not available

`pre-commit` refuses every commit, on every branch, while the service is down. Without the service, a checkpoint records nothing, an amend loses its note, and nothing reports it later.

```sh
git-ai bg start
```

An agent must not commit inside a sandbox that blocks the service.

### Checkpoints do nothing: the repository is not allowed

git-ai ignores every checkpoint in a repository that it does not allow. When `allow_repositories` has entries, git-ai matches them against remote URLs only, so a repository with no remote is not allowed, not even by `*`. Add a remote whose URL matches, or add a pattern for the remote's URL to `allow_repositories`. `prov doctor` reports this.

### A commit is refused in a contended worktree

Two sessions are Open in one worktree. End one of them, or archive it:

```sh
prov session list
prov session archive <id>
```

Changes that `prov` cannot give to one session, because they were made while the worktree was contended, become doubtful lines at the gate. A change that a session's own git-ai hook recorded keeps that session.

### The merge fails: a session is not Archived

Every session that made a commit must be Archived before its branch merges.

- **Open or stale:** `prov session archive <id>`. It waits for the agent process to exit.
- **Failed:** `prov session show <id>` shows the error. `prov session archive <id>` runs the archive step again.
- **No record:** the session's records are local to the clone that ran it. Merge from that clone.

### `prov session archive` warns that it does not know the process ID

It archives at once, and the archive can miss the agent's last work. If the agent is still running, let it finish first. Its next event returns the session to Open, and its next end archives the full transcript.

### A session record is left over after a merge

`prov doctor` names it with a `prov session drop <id>` command. A record stays when the merge commit does not have its trailer group, for example after you edited the merge message, or when the session made no commit.

### A commit has no git-ai note

git-ai writes a note for every commit that its service sees. A commit with no note was made while the service was down, or by a rewrite git-ai did not see.

- If `prov doctor` finds an old commit with the same patch and a note, an amend or a rebase made the new one. On a branch that no one else has, make the rewrite again from the old commit while the service runs; git-ai then moves the note.
- On a public branch, [replace the authorship of the files](copying.md#replacing-the-authorship-of-a-file).
- To accept the gap instead, add the commit ID to `known_gaps` in `.provenance.toml`. `prov doctor` then lists it as a note.

### A fast-forward moved a human branch onto agent commits

`post-merge` prints a warning and the command to undo it:

```sh
git reset --keep ORIG_HEAD
```

Then merge with `git merge` (which makes a merge commit, because of `merge.ff = false`).

### git-ai is newer than the tested version

`prov` depends on how git-ai attributes lines, and a change there can make the gate show wrong authors with no error. `prov doctor` warns. Use git-ai 1.7.6 until `prov` is tested with the new version.

## Limits

- **Hooks cannot stop an agent that wants to avoid them.** The controls that count are the merge gate and `prov check` in CI.
- **Agent commits are told apart by author email.** An agent that uses a person's email passes as a person. Require signed commits on human branches in your forge for a stronger control.
- **Background processes are not swept.** A process that writes after the session ends, such as a development server, has its changes taken by the next session in the worktree. Stop it before another session starts there and before `prov finish`.
- **The process scan is partial.** `prov` lists the processes whose current directory is in the worktree. A process that writes there from another directory is not found.
- **The merge message is not checked after the editor.** If you remove a trailer group there, `prov check` reports the commit, and the session records stay so you can merge again.
- **git-ai attributes whole lines.** A one-character AI change makes the whole line AI. git-ai records nothing for binary files, changes of mode, lock files, vendored paths and the other paths on its ignore list; the gate makes each such file one item. It also makes each file that an agent deletes one item.
- **Transcript archives are local files.** `prov` records their hashes in git but does not back them up or publish them.
- **macOS is not tested yet.**
