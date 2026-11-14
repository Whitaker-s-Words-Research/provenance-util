#!/bin/bash
# The checks, inside the container. $1 is the commit to check, or `worktree`
# and then the commit that the worktree has checked out.
# The git directory is mounted read-only at /src, and the checks run on a
# fresh clone of the commit, so uncommitted changes and the host's target/
# play no part. With `worktree`, the files of the working tree come in as
# /tree.tar, without target/.
set -euo pipefail
commit=$1
step() { printf '\n=== %s\n' "$*"; }

git config --global --add safe.directory '*'
git clone -q --no-checkout /src /home/ci/work
cd /home/ci/work
if [ "$commit" = worktree ]; then
  git -c advice.detachedHead=false checkout -q "$2"
  # The working tree as it is: the tracked files go, so that a file deleted
  # in the worktree is deleted here too, and the tar brings the rest.
  git ls-files -z | xargs -0 rm -f
  tar -xf /tree.tar
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
