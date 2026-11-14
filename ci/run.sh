#!/bin/bash
# The local checks: build, clippy, all tests (with git-ai) and the TLA+
# models, in a clean Podman container.
#   ci/run.sh [<commit>]    a commit; the default is HEAD
#   ci/run.sh --worktree    the working tree as it is, uncommitted changes included The pre-push hook (ci/pre-push) runs this for
# each commit that a push sends.
set -euo pipefail
root=$(git rev-parse --show-toplevel)
tree_mount=()
base=
if [ "${1:-}" = --worktree ]; then
  commit=worktree
  base=$(git rev-parse --verify 'HEAD^{commit}')
else
  commit=$(git rev-parse --verify "${1:-HEAD}^{commit}")
fi
git_dir=$(git rev-parse --path-format=absolute --git-common-dir)
image=localhost/prov-ci

# The image changes only when ci/Containerfile or rust-toolchain.toml does.
# The build context holds only these two files, not the repository.
context=$(mktemp -d)
trap 'rm -rf "$context"' EXIT
cp "$root/ci/Containerfile" "$root/rust-toolchain.toml" "$context/"
podman build -q -t "$image" -f "$context/Containerfile" "$context" >/dev/null

# With --worktree, the files go in as a tar made on the host. Reason: in a
# linked worktree, .git is a file that points to a host path, so git cannot
# list the files in the container. The tar holds what git tracks or would
# track, without what git ignores (target/).
if [ "$commit" = worktree ]; then
  (cd "$root" && git ls-files -z --cached --others --exclude-standard \
    | tar -cf "$context/tree.tar" --null -T - --ignore-failed-read)
  tree_mount=(-v "$context/tree.tar:/tree.tar:ro")
fi

# The cargo registry is cached between runs; the build itself starts clean.
podman volume exists prov-ci-cargo-registry || podman volume create prov-ci-cargo-registry >/dev/null
podman run --rm --init \
  -v "$git_dir:/src:ro" "${tree_mount[@]}" \
  -v "$root/ci/steps.sh:/home/ci/steps.sh:ro" \
  -v prov-ci-cargo-registry:/home/ci/.cargo/registry \
  "$image" /home/ci/steps.sh "$commit" $base
