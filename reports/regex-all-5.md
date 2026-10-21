# Bounded regex enumeration

`regex.find_all(re, text, max_matches, work_limit)` collects nonoverlapping
absolute UTF-8 byte spans using the existing ordered Pike VM. Language rules,
TIR/LIR, runtime ABI, pattern syntax, `find` and `is_match` are unchanged.
The full public contract and accounting are in the regex section of
`docs/spec/std-library.md`; the runnable token extraction example is
`conformance/SEM/SEM-3-regex-all-5-extract-run.hir`.

## Sources and intentional deviations

Go source pin: `ebe2feefc400f813aefdc8a036e66d2619da6649`.
The reused checkout `/tmp/ref-go` and
`/workspace/code/hirsel-briefs/pa-codec-small/refs/go` have this same identity.

- `src/regexp/regexp.go:746-802`, `matches` (the former `allMatches`):
  preserve `pos`, previous-match end, acceptance, empty-match suppression,
  rune advancement and one EOF boundary. `:948-969`, `allIndex` and
  `allStringIndex`, and `:1098-1107`, `FindAllIndex` and `FindAllStringIndex`,
  provide the collection structure.
- `src/regexp/find_test.go:29-147`: port all 81 vectors expressible in the
  current syntax with valid UTF-8, keeping whole-match spans and max counts.
  The upstream zero/max cases, Unicode, nullable repetitions, alternatives,
  anchors, and word-boundary vectors are included. Seven valid-UTF-8 rows
  need flags or POSIX classes and six rows need invalid UTF-8; these existing
  syntax/input exclusions are not new deviations.
- `src/regexp/all_test.go:607-631`: port the no-match and ten-match `a+b+`
  cases (benchmark inputs used as output assertions).
- Add 26 independent edge vectors for lazy/greedy and alternative order,
  adjacent empty/nonempty matches, anchors and boundaries after restart,
  Unicode scalar widths and match limits. Total: 109 differential vectors
  in 28 small interpreter-budget-safe programs.
- Cross-check against Rust regex `80b9d14e4be9f0b703a742d11c064abce05f210b`,
  `regex-automata/src/util/iter.rs:615-680`: empty matches must not overlap
  the preceding match, and progress must continue. Rust can advance one byte
  and let its UTF-8 engine reject interior positions. This port follows Go's
  scalar advancement directly.

Intentional implementation/API differences from Go:

1. `max_matches` is unsigned and always bounded; no negative unbounded
   sentinel. Zero returns an empty Hirsel list without examining input.
2. `Result[List[Span], MatchError]` reports deterministic cumulative work
   exhaustion without a partial result. Go has no caller work budget. Original
   VM charges are retained and all enumeration setup/progression is charged.
3. Materialize characters and byte offsets once because the existing source
   scalar view is a forward cursor. Restarts use scalar indices with original
   previous/next context, never a sliced suffix. This costs O(input scalars)
   memory and charges the entire index even for max 1. The original `find`
   remains lazy and byte-for-byte unchanged.
4. Initialize a fresh VM queue at each restart and charge every initialized
   slot. Go reuses an internal machine. This keeps the existing source VM
   contract simple, bounded and independently auditable.
5. Return whole spans only: captures and invalid-UTF-8 strings remain outside
   the existing Hirsel surface. No regex syntax, intrinsic or host hook added.

## Reproduce the independent oracle

The oracle builds **pinned Go regexp and regexp/syntax sources**, replacing
only their import path so they compile as a temporary module. It uses the Go
compiler as a build tool, not its installed regexp implementation.

```sh
python3 crates/hirsel-driver/tests/fixtures/regex-all-5-oracle.py \
  /tmp/ref-go /path/to/go
```

Executed with Go 1.25.1 linux/amd64: 109/109 fixture spans agree.
`regex-all-5-vectors.json` records exact pattern, text, max and expected spans;
`oracle_index` preserves the ordering of upstream vectors followed by the
all_test inputs and extra cases. The conformance programs assert those same
results through ordinary Hirsel source.

## Private test files

- `conformance/SEM/SEM-3-regex-all-5-go-001-run.hir` through
  `SEM-3-regex-all-5-go-028-run.hir`, each with its `.hir.json` manifest.
- `conformance/SEM/SEM-3-regex-all-5-{limits-run,extract-run}.hir`
  and their `.hir.json` manifests.
- `conformance/PROV/PROV-1-regex-all-5-position-{bad,ok}.hir`
  and their `.hir.json` manifests.
- `conformance/OWN/OWN-2-regex-all-5-take-{bad,ok}.hir` and manifests.
- `crates/hirsel-driver/tests/it-stdlib/stdlib_regex_all_5.rs`, registered by
  an append-only module row in `stdlib_patterns.rs`.
- `crates/hirsel-driver/tests/fixtures/regex-all-5-{vectors.json,oracle.py}`.

The limits fixture checks 0/1/N, immediately below/at exact completion,
final no-match proof, all-or-error behavior on 32 matches, and reusability
with the original 8/9-unit `find` boundary. The extraction fixture returns
integer spans after an arena-local source dies, then uses checked positions
and independent `unique` tokens. Negative controls reject use after explicitly transferring the input and
positions from a different string. A transfer itself is allowed because
integer spans do not retain the text.
The private logtail importer test executes its three existing package tests
on H, LLVM and Cranelift without changing the example. The driver control requires source owners and no builtin operation for all
reachable compile/find_all exports.

## Executed observations

Baseline before implementation: 1 intended conformance case executed and
failed with LANG-4, unresolved `regex.find_all`; original log retained as
`/tmp/regex-search-all-5-before.log`. Temporary parser/name/type inference
errors and a formatter-script error were corrected using ordinary language
features. No compiler or validator change was needed.

The formatter-overwrite attempt remains recorded in
`/tmp/regex-search-all-5-enumeration.log`. The source-owner attempt in
`/tmp/regex-search-all-5-source-owner.log` executed zero tests: its upload
failed because an input changed during the command (2844 versus 2410 bytes).
Neither attempt is credited as verification. Final retries use distinct logs
and an archived SHA-256 manifest of all 10,739+ consumed repository files,
with no source edits during the verification chain.

The first frozen conformance pass executed 127 cases: 126 passed; the limits
fixture used a shadowed local in its second compile arm. The two negative
controls also needed to live under their PROV rule directory. These fixture
issues were corrected before the next frozen run; original failures remain
in `/tmp/regex-search-all-5-final-conformance.log`. No VM change was required.

The second frozen run preserved 126 passing cases. The limits fixture then
needed explicit list types and copies for repeated expected results. Accepting
twins were added for the negative controls. A driver compile attempt had zero
tests because build inputs had not yet been regenerated after adding its
literal logtail include; `sync.py --if-stale` regenerates only ignored inputs.
These observations are retained in `/tmp/regex-search-all-5-retry3-*` and
`/tmp/regex-search-all-5-retry4-*`; subsequent checks select only the repaired
limits/PROV fixtures, driver controls and as-yet unverified native cases.

Final preserved results (no broad rerun after fixture-only corrections):

- 109/109 pinned Go differential vectors agree (`retry5-oracle.log`).
- 126 conformance cases passed in `retry3-conformance.log`: 96 existing regex
  files, 29 new enumeration/extraction files and the existing SER text importer.
- The repaired limits file passed via the captured CLI and as the one executed
  case in `retry5-conformance.log`. That target still returned nonzero for
  unrelated negative-control header setup, so this is per-case credit only.
- Position origin bad/ok cases passed in `retry6-conformance.log`; its third
  green was a transfer control later replaced by the stronger use-after-move
  control. Final OWN bad/ok controls passed 2/2 in `retry7-ownership.log`.
  Final corpus coverage: 131 distinct intended programs, including 34 new
  private fixtures, with all expected outcomes observed across these runs.
- `retry7-driver.log`: logtail's three package tests passed on H, LLVM and
  Cranelift (9 executions). The source-owner assertion initially expected
  unreachable old exports in a program using only compile/find_all; the final
  assertion requires just those reachable source bodies. This fixture change
  does not alter std source or invalidate the native greens.
- `retry7-native.log`: gate-native-cranelift-changed passed 127/127 exact IDs
  (96 existing regex programs, 30 new run programs, one SER importer). Its
  matrix in `crates/hirsel-conformance/src/native/cranelift.rs:35-59` executes
  LLVM O0 diagnostic **and** Cranelift diagnostic, once each. All new native
  vectors, work limits and extraction/lifetime observations passed both.
- The broader smoke action was cancelled on coordinator instruction. It is
  not credited as green and is not repeated. Read-only inspection showed
  actual compiler work: at the re2-043 sample its main thread had 21.5 CPU
  seconds and an active compiler thread was sampled at 7.87 CPU seconds.
  Those are partial observations, not an exact completed per-case duration.
  About 3,267 CPU seconds were charged to the **whole** action, not re2-043;
  it had advanced to re2-051 before cancellation. Phase observed: compile.
  The completed LLVM/Cranelift parity target measured re2-043 at its own
  per-case duration of 3.586 seconds (LLVM O0 diagnostic and Cranelift
  combined); no single-case performance rerun.

Source/compiler/runtime identities used for the completed tests:

- Source base: `2b0e271fc5e7de39bb38f60e5bb90efec3b1da96`.
- Compiler crate tree at base: `32fdd448423e11628320796f28a74ba152a90305`;
  production compiler source was unchanged by this lane.
- std@1 manifest SHA-256:
  `f3fb8ca86e75353ad33cf6d7800d7b937d20d8aaa12e7fcd695410a7be01765e`.
- Final regex.hir SHA-256:
  `acdbd46cffe27a372ae1a1a4bb63c629b397c04ecccd9e1d0b2bd071b5794cd6`.
- Runtime ABI 37; hirsel-rt source tree
  `2d037648cf3571a6af601cbacc49fee78c62205e`; rt-operations tree
  `7f45c97814a3fa66fa64a6ad19e36ac5be3c7c3a`.
- Rust toolchain 1.99.0, LLVM 21.1.8, Cargo.lock SHA-256
  `92340afd5c0ed419c264945aa54d4e25e7a03d84d9b0ea25ea9e7c357ed7feb2`.
- Frozen native repository manifest: 10,743 files, SHA-256
  `c3a5ca347bb9392c4ac62241d5dd98fc555fe586b37a4b16e48ebcb1ef14f7cf`.
  Archived as `/tmp/regex-search-all-5-snapshot-7.json`; verified unchanged
  through cancellation. Additional final test/report edits have their own
  snapshot and do not change the executed std/compiler/runtime identities.

Final source-owner retry: `source-owner-final.log`, 1 intended test executed,
1 passed, 418 filtered out. It requires both reachable compile/find_all source
owners, rejects a builtin registry implementation, and executes the arena
lifetime/extraction example. The prior logtail importer green is preserved.
The final assertion/report snapshot was archived as
`/tmp/regex-search-all-5-snapshot-8.json`, SHA-256
`968e8810c4cb69d75875a7d9a7e6c5049c43161150ad9667ce6f81473633b95a`,
and checked unchanged after the test. No remaining regex implementation
failure or compiler handoff; only the separately routed smoke compile-cost
observation remains outside this API change.
