#!/usr/bin/env bash
# Model-check cross-worker storage return (crates/hirsel-rt/src/remote.rs): the loom models over
# the runtime's own source. The pass run must find no violation; each mutation removes one
# protocol ingredient and must fail, which shows the passing run is not vacuous.
#
# Usage: check.sh [name-filter]   (a grep -E pattern over run names)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
filter="${1:-.}"

# name | cargo features | expected (pass, or fail)
loom_runs=(
  "loom||pass"
  "mutation-loom-direct-dequeue|--features direct-dequeue|fail"
  "mutation-loom-blind-trim|--features blind-trim|fail"
)
failed=0
total=0
printf '%-44s %-8s %s\n' run expected result
for row in "${loom_runs[@]}"; do
  IFS='|' read -r name features expected <<<"$row"
  [[ "$name" =~ $filter ]] || continue
  total=$((total + 1))
  set +e
  # shellcheck disable=SC2086
  out="$(cd "$here/loom" && CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$here/loom/target}" \
    cargo test --release --locked --offline $features 2>&1)"
  code=$?
  set -e
  if [[ $code -eq 0 ]]; then verdict=pass; else verdict=fail; fi
  passed="$( (grep -oE '[0-9]+ passed' <<<"$out" || true) | awk '{n += $1} END {print n + 0}')"
  if [[ "$verdict" == "$expected" ]]; then result="ok ($verdict, $passed models passed)"; else result="FAILED ($verdict)"; fi
  printf '%-44s %-8s %s\n' "$name" "$expected" "$result"
  if [[ "$result" != ok* ]]; then
    failed=$((failed + 1))
    printf '%s\n' "$out" | tail -40 | sed 's/^/    /'
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
