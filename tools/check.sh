#!/usr/bin/env bash
# Per-push gate on Buck2, with actions on the remote pool when one is configured (tools/buck2/README.md):
# changed tests, bounded agreement and native smoke. Full native matrices,
# unrestricted campaigns and tests taking about a minute belong to --full.
# Expensive crate tests retain their assertions and inputs, use
# #[ignore = "full gate"] and a full_gate_ name, and run explicitly in both
# profiles below; tools/check-native.sh runs the other ignored tests
# (tools/tests/ignored_tests.py). Expectation rewrites run under HIRSEL_BLESS
# through tools/bless.sh.
# The full gate reports every stage and fails after the last; the per-push gate
# stops at the first failure.
set -Eeuo pipefail
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" HIRSEL_JOBS="${HIRSEL_JOBS:-4}" RUST_TEST_THREADS="${RUST_TEST_THREADS:-4}"
cd "$(dirname "$0")/.."
stage="arguments"
stage_start=$SECONDS
gate_start=$SECONDS
fail_stage() {
  local status=$?
  echo "check.sh: ${stage} failed (exit ${status}, $((SECONDS - stage_start)) s)" >&2
  exit "$status"
}
trap fail_stage ERR
full=false
case "${1:-}" in
  "") ;;
  --full) full=true ;;
  --changed) ;;
  *) echo "usage: tools/check.sh [--changed|--full] (default: changed tests)" >&2; exit 2 ;;
esac
if (( $# > 1 )); then echo "usage: tools/check.sh [--changed|--full] (default: changed tests)" >&2; exit 2; fi
# The per-push gate stops at its first failing stage. The full gate runs every
# stage, so one red stage cannot hide the rest, and fails at the end.
failed_stages=()
first_status=0
run_stage() {
  stage=$1
  shift
  stage_start=$SECONDS
  echo "check.sh: ${stage}"
  if ! "$full"; then
    "$@"
  else
    local status
    # A subshell outside any `||`, so errexit still stops the stage's own
    # commands; the ERR trap is off while it runs.
    trap - ERR
    set +e
    (set -e; "$@")
    status=$?
    set -e
    trap fail_stage ERR
    if (( status != 0 )); then
      echo "check.sh: ${stage} failed (exit ${status}, $((SECONDS - stage_start)) s)" >&2
      failed_stages+=("$stage")
      (( first_status != 0 )) || first_status=$status
      return 0
    fi
  fi
  echo "check.sh: ${stage} passed ($((SECONDS - stage_start)) s)"
}
# The full gate's campaign stages are targets only under hirsel.full (tools/buck2/defs.bzl).
config=()
if "$full"; then
  # A fresh seed for the randomly seeded tests (TEST_SEED in tools/buck2/defs.bzl),
  # so the full gate never reuses a cached pass under the per-push seed.
  seed=${HIRSEL_TEST_SEED:-$(od -An -N4 -tu4 /dev/urandom | tr -d ' ')}
  echo "check.sh: randomly seeded tests run with PROPTEST_RNG_SEED=${seed}"
  config=(-c hirsel.full=true -c "hirsel.test_seed=${seed}")
fi
export HIRSEL_BUCK2_CONFIG="${config[*]}"
buck2() { local command=$1; shift; .buck2/bin/buck2 "$command" "${config[@]}" "$@"; }
release=(--target-platforms //tools/buck2:release)
# Buck2's test runner allows a test ten minutes; a loaded pool needs more for
# the per-push tests, and the campaigns take hours.
timeout=(--timeout 1800)
long=(--timeout 14400)
# The libtest arguments `cargo test -- <args>` would pass, for Buck2's test runner.
libtest() { local argument; for argument in "$@"; do printf -- '--test-arg=%s\n' "$argument"; done; }
crate_tests() { buck2 uquery '(kind("_rust_test", //...) + attrfilter(labels, doctest, //...)) - attrfilter(labels, gate, //...)'; }
# Each profile must cover the compiled inventory once across pool and host.
full_crate_tests() {
  python3 tools/buck2/full_gate_counts.py --run .buck2/bin/buck2 test "${config[@]}" "$@" -- "${long[@]}" $(libtest full_gate_ --ignored)
}
run_stage "gate orchestration tests" python3 tools/tests/main_full_run.py
run_stage "Buck2 host routing tests" python3 tools/tests/buck2_sync.py
run_stage "Buck2 prelude overlay guards" python3 tools/tests/prelude_overlay.py
run_stage "Buck2 test selection tests" python3 tools/tests/buck2_launcher.py
run_stage "full gate coverage tests" python3 tools/tests/full_gate_counts.py
run_stage "every ignored test runs in a gate" python3 tools/tests/ignored_tests.py
run_stage "Buck2 toolchains and pool configuration" python3 tools/buck2/prepare.py
run_stage "Buck2 build files match Cargo" python3 tools/buck2/sync.py --check
# Only the verdict matters here: the outputs stay on the pool, saving about
# 600 MiB of downloads per edit. Later stages fetch what
# they read. Every first-party target is in the root package: `//:` builds them
# all, without the third-party package's per-platform build-script aliases
# that `//...` adds and nothing uses.
run_stage "workspace build" buck2 build //: --materializations none
# Use the workspace's existing lint policy; no new lint-level overrides.
run_stage "workspace clippy" python3 tools/buck2/clippy.py
run_stage "formatting" python3 tools/buck2/fmt.py --check
if "$full"; then
  mapfile -t tests < <(crate_tests)
  mapfile -t libtests < <(buck2 uquery 'kind("_rust_test", //...) - attrfilter(labels, gate, //...)')
  run_stage "workspace tests" buck2 test "${tests[@]}" -- "${timeout[@]}"
  run_stage "full gate crate tests" full_crate_tests "${libtests[@]}"
else
  run_stage "changed tests and their reverse dependencies" python3 tools/buck2/changed_tests.py
  # The full gate round-trips every accepted program; per push, a seeded
  # sample re-derives every kind of signature fingerprint, beside the forged
  # export controls (PS-10).
  run_stage "proof export: seeded sample re-derives every fingerprint" \
    buck2 test //:hirsel-driver-test-it-shard-1 -- "${timeout[@]}" $(libtest proof_export::)
fi
# Sixteen shards of the corpus by path hash; the script prints their summed counts.
run_stage "conformance validation and ratchet" python3 tools/buck2/conformance.py
if "$full"; then
  # The whole seed-1 catalogue in shards, each under its own timeout (AGREE_FULL_TIMEOUT).
  run_stage "source and IR checker agreement (unrestricted)" python3 tools/buck2/agree.py --full
else
  # Fixed-seed, ranked sample.
  agree_limit=$(cd tools/buck2 && python3 -c 'from sync import AGREE_LIMIT; print(AGREE_LIMIT)')
  run_stage "source and IR checker agreement (${agree_limit} variants)" python3 tools/buck2/agree.py
fi
run_stage "native smoke: heap, x86-64 and real Io replay" buck2 test //:gate-native-smoke -- "${timeout[@]}"
if "$full"; then
  run_stage "release tests" buck2 test "${release[@]}" "${tests[@]}" -- "${timeout[@]}"
  run_stage "full gate crate tests in release" \
    full_crate_tests "${release[@]}" "${libtests[@]}"
  run_stage "native and Lean checks" tools/check-native.sh
  native_shards=$(cd tools/buck2 && python3 -c 'from sync import NATIVE_FULL_SHARDS as n; print(*(f"//:gate-native-full-shard-{i}" for i in range(1, n + 1)))')
  # shellcheck disable=SC2086 # one target per word
  run_stage "native matrix: both targets and named exclusions" buck2 test $native_shards -- "${long[@]}"
  run_stage "agreement campaign on heap and native code (unrestricted)" \
    buck2 test //:gate-agree-native-full -- "${long[@]}"
  run_stage "interpreter passing corpus with independent second hash seed" \
    buck2 test //:gate-conformance-hash-seed -- "${long[@]}"
  run_stage "native passing corpus with independent second hash seed" \
    buck2 test //:gate-native-hash-sweep -- "${long[@]}"
fi
if (( ${#failed_stages[@]} > 0 )); then
  stage="the full gate"
  printf 'check.sh: %d failing stages: %s\n' "${#failed_stages[@]}" "$(IFS=';'; echo "${failed_stages[*]}")" >&2
  echo "check.sh: failed ($((SECONDS - gate_start)) s)" >&2
  trap - ERR
  exit "$first_status"
fi
echo "check.sh: passed ($((SECONDS - gate_start)) s)"
