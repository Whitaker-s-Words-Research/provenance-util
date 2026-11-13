# Copying files

When a file moves to another repository, its git-ai line attribution stays behind, and the copy is credited to whoever commits it. `prov cp` copies files and gives every line an author.

```sh
prov cp --from ../other-repo src/parse.py src/parse.py
prov cp --from ../other-repo src/ src/                    # a directory
prov cp --from ../other-repo --rev v1.2 a.py b.py lib/    # several files into a directory
```

- `--from` is a local clone. Files are read from a commit (`--rev`, default `HEAD`), never from its working tree. Nothing in the source changes.
- A directory copies each file under it that the source commit tracks.
- With one source file, `<dest>` is the new path, or a directory if it ends with `/` or exists. With one source directory, `<dest>` is the new directory. With several sources, `<dest>` is a directory.
- It refuses if a destination file exists.
- It keeps the executable bit. Binary files and symbolic links are copied as they are, with no line authorship.
- Only a person can run it.

## The plan

`prov cp` first makes a plan: each file, with an author for each range of lines. Every line must have an author before the plan is applied, because git-ai would otherwise attach a line with no author to a nearby AI edit or to the person who copies.

The authors come from these sources, in order:

1. `--all human` or `--all ai [--agent <tool>] [--model <model>] [--session <id>]` sets every line and ignores the source's record. Use it when the source has no git-ai record, or a wrong one.
2. Without `--all`, lines that an AI session wrote in the source keep that session: its tool, model and conversation ID.
3. `--missing human` or `--missing ai [--agent <tool>] [--model <model>]` sets every line that still has no author.
4. Any line left is unresolved.

A `human` line is attributed to you, the person who copies, also when another person wrote it in the source. Only AI sessions are carried over.

If lines are unresolved and you are at a terminal, `prov cp` opens the plan in your editor (`GIT_EDITOR`, `core.editor`, `VISUAL` or `EDITOR`), as `git commit` does. Set the authors, split ranges if you need to, save and close. Without a terminal, or with lines still unresolved, it names them and stops without writing anything.

```toml
[source]
repo = "/home/me/other-repo"
commit = "4f2a9c1e..."

[[file]]
path = "src/parse.py"
dest = "src/parse.py"
ranges = [
  { lines = "1-4",   author = "human" },
  { lines = "5-9",   author = "ai", agent = "claude", model = "m1", session = "s1" },
  { lines = "10-12", author = "?" },          # unresolved
]

[[file]]
path = "img/logo.png"
dest = "img/logo.png"
binary = true
```

- The ranges of a file cover every line once.
- One session can have only one model.

For scripts, or when an agent prepares a plan for you to check:

```sh
prov cp --from ../other-repo src/ src/ --plan plan.toml   # write the plan and stop
prov cp --apply plan.toml                                 # apply it
```

## Committing a copy

`prov cp` writes each file in stages, with a git-ai checkpoint after each, and stages the result. Then you commit. The commit can hold several copies and other changes.

- **Commit a copied file as it is.** `pre-commit` refuses a commit in which a copied file differs from the copied bytes. Change it in a later commit.
- The commit gets a `Copied-From` trailer for each file: `<source>@<commit>:<path>`, followed by ` => <new path>` if the path changed. `<source>` is the source's remote URL, or its absolute path if it has no remote.
- After the commit, a background process waits for git-ai's note and compares it with the plan. If they agree, the plan is deleted. If not, `prov doctor` reports the file.
- If identical lines in a file got different authors, git-ai cannot tell which copy each stage added, and the authors can move between those lines. The counts stay correct. `prov doctor` shows a note.
- git-ai keeps no line authorship for some paths, for example `vendor/` and lock files. A copy to such a path has the trailer only, and `prov doctor` shows a note.

The transcripts of copied sessions stay in the source repository. Follow `Copied-From` to find them.

## Replacing the authorship of a file

No git-ai command changes an existing note, and git-ai attributes only the lines that a commit changes. To correct the authorship of a committed file, remove it and copy it back from your own history:

```sh
git rm src/parse.py && git commit -m "Remove parse.py to re-attribute it"
prov cp --from . --rev HEAD~1 src/parse.py src/parse.py --all ai --agent claude --model <model>
git commit -m "Restore parse.py with corrected authorship"
```

Use `--all`, or a plan that you edit. The cost: one commit in the history lacks the file, and `git blame` shows the second commit for every line. Do this only when an authorship is wrong or a copy check failed.

If the note is missing because an amend or a rebase happened while the git-ai service was down, first try to make the rewrite again while the service runs. See [Troubleshooting](troubleshooting.md#a-commit-has-no-git-ai-note).
