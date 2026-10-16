# native-tests/regressions

The native differential gate's checked-in state.

- `exclusions.tsv`: every baseline-passing executable corpus case and matrix
  configuration (by its full name; `snapshot` when the case has no native
  snapshot) that does not agree, with its class and reason. The gate requires
  the disagreeing pairs to be exactly these rows: an unlisted pair, a changed
  reason and a row whose configuration now agrees all fail, so a row for one
  configuration never hides another of the same engine. A mismatch's reason is
  its observation with a digest, so a listed wrong answer that changes fails
  too. Classes:
  `m2-unavailable`, `native-unavailable` (a stage reports the feature
  unsupported), `compiler-error` (a stage validator refused a pass's output, or
  a pass failed), `known-bug` (the engine ran and disagreed, or the heap
  executor found a violation). Rows are unmet promotion obligations, not
  waivers. After fixing something, regenerate and read the diff:
  `native --full --bless`, run by `tools/bless.sh exclusions`.
- `agree-known.txt`: the kinds of discrepancy the `agree --native full`
  campaign finds in valid variants of supported programs. A new kind fails the
  gate, and so does a listed kind that no longer occurs in a complete run of
  every shard (`tools/buck2/agree.py --native-full`).
- `smoke.txt`: the cases `tools/gate` runs per push (heap executor with
  fault injection and an optimized x86-64 executable on the diagnostic and on
  the shipped runtime archive), with eight generated successes and eight
  directed failures.
- `memcheck.txt`: corpus cases that allocate and free counted heap blocks or
  region and arena storage, which `native --sanitize` runs beside `smoke.txt`
  under Valgrind memcheck on the system-allocator archive, where each fixed-size
  block is one libc allocation, and on the memcheck archive, whose client
  requests describe the size-class allocator's slab blocks, region reservations
  and arena blocks.
- `*.hir`: programs for defects the gate found, each named in `smoke.txt`.
- `open/INDEX.tsv`: every open reason of `exclusions.tsv` with its row count and
  shortest corpus case.
- `open/*.hir`: one minimal valid program per open defect the gate holds, with
  the discrepancy in its first lines. They are not run by any gate (each is
  represented by rows in `exclusions.tsv` or a kind in `agree-known.txt`);
  reproduce with `hirsel run --native FILE` or the campaign below.
  Delete a file when its defect is fixed.

The `agree --native full` campaign, from the repository root:

```bash
archives="$(.buck2/bin/buck2 build //:hirsel-rt-archives --show-full-simple-output)"
agree="$(.buck2/bin/buck2 build --target-platforms //tools/buck2:release //:hirsel-conformance-bin-agree --show-full-simple-output)"
HIRSEL_RUNTIME_ARCHIVES="$archives" "$agree" --native full
```

The scheduled gate runs it in shards (`python3 tools/buck2/agree.py
--native-full`).
