#!/usr/bin/env bash
# Model-check the free-mode protocol (crates/hirsel-free): the loom models over the kernel's own
# section, cutter and claim source, and the GenMC models of the SC section handshake, the claim
# protocol and causal closure under RC11 (and IMM for the gate).
#
# Each "pass" run must find no violation; each "mutation" run removes one protocol ingredient
# and must fail, which shows the passing runs are not vacuous.
#
# GenMC is built once, pinned by commit, into the cache (like the TLA+ jar of proofs/ordered):
# it needs cmake, clang/LLVM 19-22 with its development files, and libffi.
#
# Usage: check.sh [name-filter]   (a grep -E pattern over run names)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
filter="${1:-.}"

genmc_commit=9f6c4c0772d0c42b325681581acc6a0f3eb9b5f7 # GenMC v0.19.0
cache="${XDG_CACHE_HOME:-$HOME/.cache}/genmc-$genmc_commit"
genmc="${GENMC:-$cache/build/bin/genmc}"
# GenMC bakes its own include path in at configure time, so it is built where it stays; the
# marker says the build finished.
if [[ -z "${GENMC:-}" && ! -f "$cache/.built" ]]; then
  llvm=""
  for v in 22 21 20 19; do
    if [[ -d /usr/lib/llvm-$v/lib/cmake/llvm ]]; then llvm=$v; break; fi
  done
  [[ -n "$llvm" ]] || { echo "check.sh: GenMC needs LLVM 19-22 development files (/usr/lib/llvm-N)" >&2; exit 1; }
  rm -rf "$cache"
  echo "check.sh: building GenMC $genmc_commit into $cache" >&2
  git clone -q https://github.com/MPI-SWS/genmc "$cache"
  git -C "$cache" checkout -q "$genmc_commit"
  cmake -S "$cache" -B "$cache/build" -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_C_COMPILER="clang-$llvm" -DCMAKE_CXX_COMPILER="clang++-$llvm" \
    -DCMAKE_PREFIX_PATH="/usr/lib/llvm-$llvm/lib/cmake" >/dev/null
  cmake --build "$cache/build" -j"$(nproc)" >/dev/null 2>&1
  touch "$cache/.built"
fi

# name | model flags | expected (pass, or fail)
genmc_runs=(
  "genmc-gate-rc11||gate.c|pass"
  "genmc-gate-imm|--imm|gate.c|pass"
  "genmc-claim||claim.c|pass"
  "genmc-claim-no-recording||-DNO_RECORDING claim.c|pass"
  "genmc-events|--disable-estimation|events.c|pass"
  "mutation-genmc-weak-enter||-DWEAK_ENTER gate.c|fail"
  "mutation-genmc-blind-withdraw||-DWITHDRAW_BLIND claim.c|fail"
  "mutation-genmc-blind-withdraw-no-recording||-DNO_RECORDING -DWITHDRAW_BLIND claim.c|fail"
)

failed=0
total=0
printf '%-44s %-8s %s\n' run expected result
for row in "${genmc_runs[@]}"; do
  IFS='|' read -r name options source expected <<<"$row"
  [[ "$name" =~ $filter ]] || continue
  total=$((total + 1))
  set +e
  # shellcheck disable=SC2086
  out="$(cd "$here/genmc" && "$genmc" $options -- $source 2>&1)"
  set -e
  if grep -q "No errors were detected" <<<"$out"; then verdict=pass
  elif grep -q "Error:" <<<"$out"; then verdict=fail
  else verdict=unknown; fi
  executions="$(sed -nE 's/Number of complete executions explored: ([0-9]+)/\1/p' <<<"$out")"
  if [[ "$verdict" == "$expected" ]]; then result="ok ($verdict, ${executions:-?} executions)"; else result="FAILED ($verdict)"; fi
  printf '%-44s %-8s %s\n' "$name" "$expected" "$result"
  if [[ "$result" != ok* ]]; then
    failed=$((failed + 1))
    printf '%s\n' "$out" | tail -40 | sed 's/^/    /'
  fi
done

# Section/claim models use the failed-poll wait abstraction documented in README.md.
# Keep the historical bound-1 run distinct from the explicit bound-2 row. Other
# model families retain their own bound-2 runs. Pin the original branch budget;
# an exhausted budget or setup failure is never an acceptable mutation result.
loom_runs=(
  "loom||1|pass"
  "loom-section-bound2|--test models|2|pass"
  "loom-schedule-bound2|--test schedule|2|pass"
  "loom-cancel-bound2|--test cancel|2|pass"
  "loom-atomic-bound2|--test atomic|2|pass"
  "loom-mutex-bound2|--test mutex|2|pass"
  "loom-channel-bound2|--test channel|2|pass"
  "mutation-loom-weak-handshake|--test models --features weak|2|causality"
)
for row in "${loom_runs[@]}"; do
  IFS='|' read -r name features preemptions expected <<<"$row"
  [[ "$name" =~ $filter ]] || continue
  total=$((total + 1))
  test_args=()
  if [[ "$expected" == causality ]]; then
    test_args=(-- --exact every_cut_is_causally_closed)
  fi
  set +e
  # shellcheck disable=SC2086
  out="$(cd "$here/loom" && LOOM_MAX_PREEMPTIONS="$preemptions" LOOM_MAX_BRANCHES=1000 CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$here/loom/target}" \
    cargo test --release --locked --offline $features "${test_args[@]}" 2>&1)"
  code=$?
  set -e
  passed="$(grep -oE '[0-9]+ passed' <<<"$out" | awk '{n += $1} END {print n + 0}' || true)"
  if [[ $code -eq 0 && "$passed" -gt 0 ]] &&
     [[ "$name" != loom-section-bound2 || "$passed" -eq 6 ]]; then
    verdict=pass
  elif [[ $code -eq 101 ]] &&
       grep -Fq 'Causality violation: Concurrent write accesses to `UnsafeCell`.' <<<"$out" &&
       grep -Fq 'test result: FAILED. 0 passed; 1 failed;' <<<"$out"; then
    verdict=causality
  else
    verdict=unknown
  fi
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
