#!/usr/bin/env bash
# Model-check the ordered-mode TLA+ model (Ordered.tla) over every scenario in MC.tla.
#
# Each "check" run must pass: TLC exits 0 with the Soundness invariant ((a)-(c), no fault,
# final equality), deadlock checking on. Each "positive" run also checks that the named early
# commits happen under eager scheduling. Each "mutation" run removes one protocol rule and must
# fail with the named invariant violated, which shows the passing runs are not vacuous.
#
# Usage: check.sh [name-filter]   (a grep -E pattern over run names)
# Exit 0 when every selected run has its expected result.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"

version=1.7.4
sha=936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88
jar="${TLA2TOOLS_JAR:-${XDG_CACHE_HOME:-$HOME/.cache}/tla/tla2tools-$version.jar}"
if [[ ! -f "$jar" ]]; then
  mkdir -p "$(dirname "$jar")"
  curl -sSfL -o "$jar.part" "https://github.com/tlaplus/tlaplus/releases/download/v$version/tla2tools.jar"
  mv "$jar.part" "$jar"
fi
if ! sha256sum --quiet -c <<<"$sha  $jar"; then
  echo "check.sh: $jar does not match the pinned tla2tools $version (sha256 $sha)" >&2
  exit 1
fi
command -v java >/dev/null || { echo "check.sh: java not found (TLC needs Java 11 or later)" >&2; exit 1; }

# name | scenario | mutations | eager | invariants | expected (pass, or the violated invariant)
runs=(
  "mix-3x4|Mix|{}|FALSE|Soundness|pass"
  "tree-mix-3x4|TreeMix|{}|FALSE|Soundness|pass"
  "close-store-load|CloseStoreLoad|{}|FALSE|Soundness|pass"
  "drop-finalizer-yields|DropFinalizerYields|{}|FALSE|Soundness|pass"
  "detach-all-yields|DetachAllYields|{}|FALSE|Soundness|pass"
  "detach-while-early-send|DetachWhileEarlySend|{}|FALSE|Soundness|pass"
  "detach-call-term|DetachCallTerm|{}|FALSE|Soundness|pass"
  "readiness-counts|ReadinessCounts|{}|FALSE|Soundness|pass"
  "early-send-parked|EarlySendParked|{}|FALSE|Soundness|pass"
  "cancel-host-waiter|CancelHostWaiter|{}|FALSE|Soundness|pass"
  "failure-store-recv|FailureStoreRecv|{}|FALSE|Soundness|pass"
  "thaw-release|ThawRelease|{}|FALSE|Soundness|pass"
  "mutex-body-thaw|MutexBodyThaw|{}|FALSE|Soundness|pass"
  "budget-review2-1|Budget2a|{}|FALSE|Soundness|pass"
  "budget-review2-2|Budget2b|{}|FALSE|Soundness|pass"
  "budget-review3-1a|Budget3a|{}|FALSE|Soundness|pass"
  "budget-review3-1b|Budget3b|{}|FALSE|Soundness|pass"
  "budget-review3-2|Budget3c|{}|FALSE|Soundness|pass"
  "pass-mutex-atomic|PassMutexAtomic|{}|FALSE|Soundness|pass"
  "pass-send-load|PassSendLoad|{}|FALSE|Soundness|pass"
  "one-turn-per-head|OneTurnPerHead|{}|FALSE|Soundness|pass"
  "logged-close-then-send|LoggedCloseThenSend|{}|FALSE|Soundness|pass"
  "handle-through-channel|HandleThroughChannel|{}|FALSE|Soundness|pass"
  # Positive: the early commits happen (eager scheduling).
  "positive-detach-while-early-send|DetachWhileEarlySend|{}|TRUE|Soundness Positive|pass"
  "positive-early-send-parked|EarlySendParked|{}|TRUE|Soundness Positive|pass"
  "positive-failure-store|FailureStoreRecv|{}|TRUE|Soundness Positive|pass"
  "positive-mutex-body-thaw|MutexBodyThaw|{}|TRUE|Soundness Positive|pass"
  "positive-mutex-atomic|PassMutexAtomic|{}|TRUE|Soundness Positive|pass"
  "positive-send-load|PassSendLoad|{}|TRUE|Soundness Positive|pass"
  # Witnesses: the generic runs reach an early commit and a logged (run-ahead) op.
  "witness-mix-early|Mix|{}|FALSE|NoEarlyCommit|NoEarlyCommit"
  "witness-mix-logged|Mix|{}|FALSE|NoLoggedOp|NoLoggedOp"
  "witness-tree-mix-early|TreeMix|{}|FALSE|NoEarlyCommit|NoEarlyCommit"
  "witness-tree-mix-logged|TreeMix|{}|FALSE|NoLoggedOp|NoLoggedOp"
  # Mutations: one rule removed, the named invariant must fail.
  "mutation-rule1|OneTurnPerHead|{\"norule1\"}|FALSE|Behavior|Behavior"
  "mutation-rule2|LoggedCloseThenSend|{\"norule2\"}|FALSE|Behavior|Behavior"
  "mutation-rule3|CloseStoreLoad|{\"norule3\"}|FALSE|Behavior|Behavior"
  "mutation-rule3-mutex|MutexBodyThaw|{\"norule3\"}|FALSE|Behavior|Behavior"
  "mutation-rule3-closure|HandleThroughChannel|{\"noclosure\"}|FALSE|Behavior|Behavior"
  "mutation-rule3-call-term|DetachCallTerm|{\"nocallterm\"}|FALSE|Behavior|Behavior"
  "mutation-rule4-pending|EarlySendParked|{\"nopending\"}|FALSE|Behavior|Behavior"
  "mutation-rule4-touch-at-commit|EarlySendParked|{\"touchatcommit\"}|FALSE|Behavior|Behavior"
  "mutation-rule4-filter|EarlySendParked|{\"nofilter\"}|TRUE|Soundness Positive|Positive"
  "mutation-cancel-early|CancelHostWaiter|{\"cancelE\"}|FALSE|Behavior|Behavior"
  "mutation-receive-early|FailureStoreRecv|{\"recvE\"}|FALSE|Behavior|Behavior"
  "mutation-release-immediate|ThawRelease|{\"immrelease\"}|FALSE|Behavior|Behavior"
  "mutation-drop-deferred|DropFinalizerYields|{\"droprecvD\"}|FALSE|Behavior|Behavior"
  "mutation-call-appended|DetachAllYields|{\"callnoinsert\"}|FALSE|Behavior|Behavior"
  "mutation-early-poll|ReadinessCounts|{\"earlysp\"}|FALSE|Behavior|Behavior"
  "mutation-head-sees-speculation|Budget2a|{\"headseesspec\"}|FALSE|Behavior|Behavior"
  "mutation-net-only|Budget2b|{\"netonly\"}|FALSE|Behavior|Behavior"
  "mutation-peak-only|Budget3b|{\"peakonly\"}|FALSE|Behavior|Behavior"
  "mutation-merged-pieces|Budget3c|{\"mergepieces\"}|FALSE|Behavior|Behavior"
)

filter="${1:-.}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cp "$here/Ordered.tla" "$here/MC.tla" "$work/"
workers="$(( $(nproc) > 8 ? 8 : $(nproc) ))"

failed=0
total=0
printf '%-36s %-8s %10s %8s  %s\n' run expected distinct depth result
for row in "${runs[@]}"; do
  IFS='|' read -r name scenario mut eager invariants expected <<<"$row"
  [[ "$name" =~ $filter ]] || continue
  total=$((total + 1))
  {
    printf 'CONSTANTS\n  Cfg <- %s\n  Mut = %s\n  Eager = %s\n' "$scenario" "$mut" "$eager"
    printf 'SPECIFICATION Spec\n'
    for inv in $invariants; do printf 'INVARIANT %s\n' "$inv"; done
  } >"$work/$name.cfg"
  set +e
  out="$(cd "$work" && java -XX:+UseParallelGC -cp "$jar" tlc2.TLC -nowarning -cleanup \
           -metadir "$work/meta-$name" -workers "$workers" -config "$name.cfg" MC.tla 2>&1)"
  code=$?
  set -e
  distinct="$(sed -nE 's/.* ([0-9]+) distinct states found.*/\1/p' <<<"$out" | tail -1)"
  depth="$(sed -nE 's/.*depth of the complete state graph search is ([0-9]+).*/\1/p' <<<"$out")"
  if [[ "$expected" == pass ]]; then
    if [[ $code -eq 0 ]]; then result=ok; else result="FAILED (TLC exit $code)"; fi
  else
    if [[ $code -eq 12 ]] && grep -q "Invariant $expected is violated" <<<"$out"; then
      result="ok (violated as expected)"
    else
      result="FAILED (expected $expected violated; TLC exit $code)"
    fi
  fi
  printf '%-36s %-8s %10s %8s  %s\n' "$name" "$expected" "${distinct:--}" "${depth:--}" "$result"
  if [[ "$result" != ok* ]]; then
    failed=$((failed + 1))
    printf '%s\n' "$out" | tail -60 | sed 's/^/    /'
  fi
done
if (( total == 0 )); then
  echo "check.sh: no run matches '$filter'" >&2
  exit 1
fi
if (( failed > 0 )); then
  echo "check.sh: $failed of $total runs did not have their expected result" >&2
  exit 1
fi
echo "check.sh: all $total runs have their expected result"
