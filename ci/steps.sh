#!/bin/bash
# The checks, inside the container. $1 is the commit to check, or `worktree`.
# The git directory is mounted read-only at /src, and the checks run on a
# fresh clone of the commit, so uncommitted changes and the host's target/
# play no part. With `worktree`, the working tree is mounted at /tree and
# copied without target/.
set -euo pipefail
commit=$1
step() { printf '\n=== %s\n' "$*"; }

git config --global --add safe.directory '*'
git clone -q --no-checkout /src /home/ci/work
cd /home/ci/work
if [ "$commit" = worktree ]; then
  git -c advice.detachedHead=false checkout -q HEAD
  # The working tree as it is, with uncommitted and untracked files, but
  # without what git ignores (target/).
  (cd /tree && git ls-files -z --cached --others --exclude-standard) \
    | (cd /tree && xargs -0 tar -cf - --ignore-failed-read) | tar -xf -
  echo "checking the working tree, on top of $(git log -1 --format='%h %s')"
else
  git -c advice.detachedHead=false checkout -q "$commit"
  echo "checking $(git log -1 --format='%h %s')"
fi

step "versions"
cargo --version; git --version; git-ai --version; java -version 2>&1 | head -1
step "build"
cargo build --workspace --locked
step "clippy"
cargo clippy --workspace --all-targets --locked -- -D warnings
step "tests"
# An integration test that cannot find git-ai fails here instead of skipping.
PROV_TEST_REQUIRE_GITAI=1 cargo test --workspace --locked
step "models"
mkdir -p tla/tools && cp /home/ci/tla2tools.jar tla/tools/
tla/check-models.sh
step "all checks passed"
