#!/usr/bin/env bash
# Rewrite checked-in expectations from the code that derives them, then review the diff.
#   tools/bless.sh <what>...
# Every bless runs on this host, so it writes into this checkout: a pool action
# reads a discarded copy, and test_launcher.sh refuses a bless there.
#
# In-test blesses run their tests with HIRSEL_BLESS=1 (snapshots: insta's
# INSTA_UPDATE=always), fail unless some case ran, and then run the same cases
# again without the bless, which fails unless the rewritten files pass:
#   heap               native-tests/heap/              heapcheck fixtures and traces
#   layout             native-tests/layout/            layout tables
#   rc-import          crates/hirsel-pipeline-tests/tests/rc/fixtures/  golden LIR dumps
#   wire               crates/hirsel-types/tests/wire/golden.tsv        SER-2 vectors
#   replay             native-tests/replay/column-skew/                 replay fixture
#   signatures         crates/hirsel-signature/src/golden.txt           encoding vectors
#   package-docs       packages/doc_api@1/api.md
#   core-review        crates/hirsel-engine/src/registry/review-table.txt
#   native-inventory   native-tests/library/operations.tsv
#   ownership-passing  crates/hirsel-ownership/src/tests/{passing,pending}.txt
#   authority-corpus   crates/hirsel-authority/src/corpus/{passing,pending}.txt
#   snapshots          insta snapshots: lowering, grammar examples, mode explanations
#   all                every in-test bless above
# Corpus-wide blesses:
#   passing            conformance/passing.txt: hirsel-conformance --update-passing (adds only)
#   exclusions         native-tests/regressions/exclusions.tsv: native --full --bless (hours)
#   native-selection   native-tests/regressions/native-selection.json: the frozen program
#                      cover, derived on the pool (-c hirsel.native_selection=derive)
set -euo pipefail
usage() {
  sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//' >&2
  exit 2
}
(( $# >= 1 )) || usage
cd "$(dirname "$0")/.."
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" HIRSEL_JOBS="${HIRSEL_JOBS:-4}" RUST_TEST_THREADS="${RUST_TEST_THREADS:-4}"
buck2=.buck2/bin/buck2
python3 tools/buck2/sync.py --if-stale
log="$(mktemp)"
trap 'rm -f "$log"' EXIT

# `<target> <libtest filter>` rows for one in-test bless. A sharded binary runs a
# filtered selection in its first shard (tools/buck2/README.md).
cases() {
  case "$1" in
    heap) echo "//:hirsel-heapcheck-test-it heap::" ;;
    layout) echo "//:hirsel-lir-test-it layout::" ;;
    rc-import) echo "//:hirsel-pipeline-tests-test-rc rc_import::" ;;
    wire) echo "//:hirsel-types-test-wire recursive_canonical_golden_vectors" ;;
    replay) echo "//:hirsel-conformance-test-it native_gate::a_stored_artifact_replays_from_another_path_and_arena_numbering" ;;
    signatures) echo "//:hirsel-signature-unittest tests::golden_vectors" ;;
    package-docs) echo "//:hirsel-driver-test-it-shard-1 api_index::package_public_api_matches_readable_golden" ;;
    core-review) echo "//:hirsel-engine-unittest registry::tests::every_declared_signature_matches_the_review_table_and_canonical_hash" ;;
    native-inventory) echo "//:hirsel-pipeline-tests-test-front rt_operations::declaration_digests_match_the_reviewed_native_inventory" ;;
    ownership-passing) echo "//:hirsel-ownership-unittest tests::corpus::source_corpus_ratchets_rule_and_primary_line" ;;
    authority-corpus) echo "//:hirsel-authority-unittest corpus::real_phase_corpus_ratchet" ;;
    snapshots)
      echo "//:hirsel-lower-unittest tests::"
      echo "//:hirsel-parser-test-it grammar_examples::"
      echo "//:hirsel-pipeline-tests-test-front modes_explanations::"
      ;;
    *) return 1 ;;
  esac
}
in_test=(package-docs heap layout rc-import wire replay signatures core-review native-inventory
  ownership-passing authority-corpus snapshots)

# Run one target's filtered cases, on this host with `--env` options when
# given, and fail unless at least one passed: a filter that matches nothing
# reports success.
run_cases() {
  local target=$1 filter=$2 config=()
  shift 2
  # Resolve the generated test target: sync may shard or unshard this binary
  # as its measured cost changes. A filtered sharded test runs in shard 1.
  local resolved
  resolved="$("$buck2" uquery "attrregexfilter(name, \"^${target#//:}(-shard-1)?$\", kind(\"_rust_test\", //:))")"
  if [[ -z $resolved || $resolved == *$'\n'* ]]; then
    echo "bless.sh: expected one test target for $target, got: $resolved" >&2
    exit 1
  fi
  target=$resolved
  (( $# == 0 )) || config=(-c hirsel.tests=host)
  if ! "$buck2" test "${config[@]}" "$target" -- "$@" --test-arg="$filter" 2>&1 | tee "$log"; then
    echo "bless.sh: $target $filter failed" >&2
    exit 1
  fi
  local passed
  passed="$(sed -n 's/.*test result: ok\. \([0-9]*\) passed.*/\1/p' "$log" | awk '{ n += $1 } END { print n + 0 }')"
  if (( passed == 0 )); then
    echo "bless.sh: no case of $target matches $filter" >&2
    exit 1
  fi
  echo "bless.sh: $target $filter: $passed passed${config[*]:+ on this host}"
}

# The bless on this host, then the same cases without it.
bless_in_test() {
  local env=(--env HIRSEL_BLESS=1) target filter
  [[ $1 != snapshots ]] || env=(--env INSTA_UPDATE=always)
  while read -r target filter; do run_cases "$target" "$filter" "${env[@]}"; done < <(cases "$1")
  while read -r target filter; do run_cases "$target" "$filter"; done < <(cases "$1")
}

for what in "$@"; do
  case "$what" in
    all) for each in "${in_test[@]}"; do bless_in_test "$each"; done ;;
    passing) "$buck2" run //:hirsel-conformance-bin-hirsel-conformance -- --update-passing --root "$PWD" ;;
    exclusions) "$buck2" test -c hirsel.full=true -c hirsel.tests=host //:gate-native-full -- --test-arg=--bless --timeout 14400 ;;
    native-selection)
      derived="$("$buck2" build -c hirsel.native_selection=derive //:native-selection --show-full-simple-output)"
      cp "$derived" native-tests/regressions/native-selection.json ;;
    *) cases "$what" > /dev/null || usage; bless_in_test "$what" ;;
  esac
done
git status --short
