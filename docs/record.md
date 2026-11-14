# The record

`prov` stores almost nothing of its own. The record is in your git history, so anyone can read it with `git log` and git-ai.

| Question | Where the answer lives |
|---|---|
| Who wrote each line | git-ai's notes on each commit (`refs/notes/ai`) |
| Which session made each commit | A `Prov-Session` trailer on the commit |
| Where the transcript is | An archive on your disk, with its hash in the merge commit |
| Who approved it | The merge commit that a person made, with `Prov-Approved` trailers |
| Who corrected merged agent work | A `Prov-Revises` trailer on the human commit, and `git blame` |
| Where a copied file came from | A `Copied-From` trailer on the commit that added it |

## A merge commit

After two sessions on one branch:

```text
Merge branch 'agent/claude/20261111-parser-tests'

Prov-Session: claude:5c1e2b9a-...
Prov-Transcript-SHA256: 9f2c...
Prov-Model: claude-fable-5-2
Prov-Session: claude:a71d04e3-...
Prov-Transcript-SHA256: 41b8...
Prov-Approved: notes.txt ack
Prov-Version: 0.1.0+<build SHA>
```

Each session has one group, in start order, starting with `Prov-Session`. Each agent commit on the branch also has its own `Prov-Session` trailer, so the commit subjects of a session are its summary.

## Trailers

| Trailer | On | Value |
|---|---|---|
| `Prov-Session` | each agent commit; each group of a merge | `<agent>:<session ID>` |
| `Prov-Transcript-SHA256` | each group | the SHA-256 of the transcript archive, 64 hex characters |
| `Prov-Model` | each group, if known | the session's latest model. git-ai's note can say `unknown`; this trailer covers it. |
| `Prov-Approved` | the commit that the approval is for | `<path> <choice>`, where the choice is `ack`, `human` or `ai` |
| `Prov-Version` | each merge of agent commits | `<version>+<build SHA>` |
| `Prov-Revises` | a commit of `prov revise` | the full ID of a merge that it revises; one trailer for each merge |
| `Copied-From` | a commit with copied files | `<source>@<commit>:<path>`, followed by ` => <dest>` if the path changed |

Every value is one line.

## Reading the record

Every merged session on a branch:

```sh
git log --first-parent --format='%h %s%n%(trailers:key=Prov-Session)' main
```

Who wrote each line of a file:

```sh
git-ai blame src/parse.py
```

The authorship of one commit:

```sh
git-ai diff <commit>
git-ai stats <commit>
```

## Transcript archives

When a session ends, `prov` archives its transcript to `llm-logs/<agent>/<session>-<hash>.tar.gz`. The directory is not tracked by git.

- For Claude Code, the archive holds the transcript as `transcript.jsonl`, and the session's directory next to it, which holds large tool results and subagent transcripts.
- The hash in the merge commit is the SHA-256 of the tar bytes, before compression. To check an archive, decompress it and hash the result:

  ```sh
  gzip -dc llm-logs/claude/<file>.tar.gz | sha256sum
  ```

- Every file in the archive has the date 1970-01-01, owner 0 and mode 0644. This is on purpose: the same transcript always gives the same archive and the same hash. The times of the messages are in the transcript itself.
- Every session that ends is archived, also one that made no commit. A resumed session gets a new archive file each time it ends; the hash in an earlier merge stays correct.
- `prov` never overwrites an archive. It does not back archives up or publish them; that is your job.

## Agent Trace export

`prov trace export [<range>]` writes the authorship record in the [Agent Trace](https://agent-trace.dev/) 0.1.0 format, as JSON Lines: one record for each commit with a note. The default range is the branch that is checked out. It is optional.

- Each record is validated against the schema pinned in `vendor/agent-trace/0.1.0/`.
- AI lines become contributor type `ai`, human lines `human`, untracked lines `unknown`.
- `model_id` is `<provider>/<model>`, for example `anthropic/claude-fable-5-2`. When the note says `unknown`, the model comes from the `Prov-Model` trailer of the session's merge.
- Two exports of the same range give identical bytes.
