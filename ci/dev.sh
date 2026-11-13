#!/bin/bash
# A cargo command in the container of the local checks, on the working tree
# as it is. For day-to-day work: the build stays in a Podman volume, so a
# second run rebuilds only what changed. Before a push, ci/run.sh builds clean.
#
#   ci/dev.sh test --workspace
#   ci/dev.sh test -p prov --test gate protected
#   ci/dev.sh clippy --workspace --all-targets -- -D warnings
#   ci/dev.sh --models            the TLA+ models
#   ci/dev.sh --clean             delete the cached build
#
# Reason: the checkout is on a slow 9p mount. In the container, the sources
# and the build are on the local disk.
set -euo pipefail
root=$(git rev-parse --show-toplevel)
image=localhost/prov-ci
target_volume=prov-dev-target

if [ "${1:-}" = --clean ]; then
  podman volume rm -f "$target_volume" >/dev/null && echo "deleted the cached build"
  exit 0
fi
[ $# -gt 0 ] || { sed -n '2,13p' "$0"; exit 2; }

context=$(mktemp -d)
trap 'rm -rf "$context"' EXIT
cp "$root/ci/Containerfile" "$root/rust-toolchain.toml" "$context/"
podman build -q -t "$image" -f "$context/Containerfile" "$context" >/dev/null
for v in prov-ci-cargo-registry "$target_volume"; do
  podman volume exists "$v" || podman volume create "$v" >/dev/null
done

# The sources go in as a tar of what git tracks or would track (not target/).
# tar keeps the modification times, so cargo rebuilds only changed crates.
(cd "$root" && git ls-files -z --cached --others --exclude-standard \
   | xargs -0 tar -cf - --ignore-failed-read 2>/dev/null) \
| podman run --rm -i --init \
    -v prov-ci-cargo-registry:/home/ci/.cargo/registry \
    -v "$target_volume":/home/ci/target \
    -e CARGO_TARGET_DIR=/home/ci/target -e PROV_TEST_REQUIRE_GITAI=1 \
    "$image" bash -c '
      set -e
      mkdir -p /home/ci/w && cd /home/ci/w && tar -xf -
      # build.rs reads the commit for Prov-Version; the container has no history.
      git init -q && git -c user.name=dev -c user.email=dev@localhost commit -q --allow-empty -m dev
      if [ "$1" = --models ]; then
        mkdir -p tla/tools && cp /home/ci/tla2tools.jar tla/tools/ && exec tla/check-models.sh
      fi
      exec cargo "$@"' dev "$@"
