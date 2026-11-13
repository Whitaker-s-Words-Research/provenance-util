#!/usr/bin/env bash
# Check every TLA+ model in this directory.
#
#   <Module>.cfg            must PASS
#   <Module>.bug-*.cfg      must FAIL with a property violation (old SPEC.md rule; regression canary)
#   <Module>.limit-*.cfg    must FAIL with a property violation (accepted, documented limit)
#   <Module>.reach-*.cfg    must FAIL with a property violation (proves a state is reachable)
#
# "Fail" means TLC exit code 12 (safety) or 13 (liveness). Any other non-zero
# exit (parse error, bad config, TLC crash) is reported as an error, so a
# broken canary can never look like a working one.
#
# Env: TLA2TOOLS = path to tla2tools.jar (default: ./tools/tla2tools.jar)
#      JAVA      = java binary (default: java)
#      VERBOSE=1 prints TLC output for unexpected results.
set -uo pipefail
cd "$(dirname "$0")"
JAR=${TLA2TOOLS:-tools/tla2tools.jar}
JAVA=${JAVA:-java}
META=$(mktemp -d)
trap 'rm -rf "$META"' EXIT
fail=0

# 1. Every module parses.
for tla in *.tla; do
  if ! out=$("$JAVA" -cp "$JAR" tla2sany.SANY "$tla" 2>&1) || grep -q '\*\*\* Errors' <<<"$out"; then
    echo "SANY FAILED   $tla"; [[ -n ${VERBOSE:-} ]] && echo "$out"; fail=1
  fi
done

# 2. Run each config.
for cfg in *.cfg; do
  mod=${cfg%%.*}
  out=$("$JAVA" -XX:+UseParallelGC -cp "$JAR" tlc2.TLC -workers auto -deadlock \
        -metadir "$META/${cfg%.cfg}" -config "$cfg" "$mod.tla" 2>&1); rc=$?
  states=$(grep -oE '[0-9,]+ distinct states found' <<<"$out" | tail -1)
  case "$cfg" in
    *.bug-*|*.limit-*|*.reach-*) want=violation ;;
    *) want=pass ;;
  esac
  if [[ $want == pass && $rc == 0 ]]; then
    echo "ok   pass     $cfg  ($states)"
  elif [[ $want == violation && ( $rc == 12 || $rc == 13 ) ]]; then
    echo "ok   violated $cfg  ($(grep -cE '^State [0-9]+:' <<<"$out") steps)"
  else
    echo "FAIL $cfg: expected $want, TLC exit $rc"; fail=1
    [[ -n ${VERBOSE:-} ]] && echo "$out"
  fi
done

# 3. Every rule ID used in a model exists in the spec.
specs=../SPEC.md
unknown=$(comm -23 <(grep -ohE 'R-[A-Z]+-[0-9]+' *.tla | sort -u) \
                   <(cat $specs | grep -oE 'R-[A-Z]+-[0-9]+' | sort -u))
if [[ -n $unknown ]]; then
  sed 's/^/Unknown rule ID in model: /' <<<"$unknown"; fail=1
fi

exit $fail
