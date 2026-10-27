# Native library evidence

Programs too large for bounded interpretation can use `// oracle: expected-stdout`
and a nonempty `// oracle-reason: ...` header with a sibling `<stem>.stdout`.
The native harness skips interpretation and heap execution, and requires each
LLVM and Cranelift executable to exit successfully and match the sidecar's exact
bytes. This opt-in is restricted to this directory; ordinary programs still use
the interpreter. `oracle-expected-stdout.hir` is a million-iteration arithmetic
control whose independently derived sum is `1000000 * 999999 / 2 = 499999500000`.

`operations.tsv` pins the 242 registry operation paths, declaration SHA-256 digests and declared ownership. The final column is `ordinal:passing:cleanup;...->result_cleanup`: b borrow, t take, m mut; n noalloc, a may allocate, [ordinals] generic cleanup dependence. Adding a registry row or changing a digest/convention requires an explicit native review and fails `hirsel-pipeline-tests/tests/front/rt_operations.rs` first. No source schema is inferred from executable code.


`arena.seal` and `region.with` use reviewed library bodies. The capture-free
heap `List[u64]` root in `conformance/MEM/MEM-4-sealed-heap-run.hir` runs on the
reference interpreter, checked H, LLVM and Cranelift, including record/replay.
`MEM-4-sealed-heap-charge-run.hir` also checks bounded evacuation: capacity 15
refuses two u64 elements, while capacities 16 and 17 preserve both elements.
`MEM-4-sealed-empty-heap-empty-run.hir` accepts an empty heap list at bound zero.
`MEM-4-sealed-arena-root-run.hir` also qualifies a capture-free Arena
`List[List[u64]]` root with a copied heap `List[u64]` child, repeated reads and
record/replay on all four engines. Its explicit, live construction receipts
pair both the original Arena destination and every heap child; physical guard
cleanup retains the child credits until seal or exit.
`MEM-4-sealed-arena-child-charge-run.hir` checks the same child custody under
bounded admission: capacities 7, 8 and 15 refuse, while capacity 16 preserves
the nested element. Rejected construction drops its taken children before
entering the caller's failure cleanup.
Original source declarations and ordered child receipts remain mandatory.
Captured closures, held environments, tasks, additional mutation recipes and
fully elided payload custody remain unsupported on this path.

`exact-sums.txt` was retained verbatim from the interpreter's 256 Python Fraction vectors (seed 20261008). `GraphemeBreakTest.txt` is the complete official Unicode 16.0.0 fixture, including its source and license header, retained verbatim from interpreter fixtures. `hirsel-driver/tests/stdlib_wave2_num.rs` checks the rational vectors against ordinary std source; `hirsel-driver/tests/stdlib_graphemes.rs` checks the shipped source over full strings and nonzero-origin slices.

Numeric algorithms execute in ordinary std source; the independent rational
exact-sum vectors are consumed by `hirsel-driver/tests/stdlib_wave2_num.rs`.
`run-c.sh` runs the C probes on the diagnostic Buck2 runtime archive, x86-64 under
ASan/UBSan and AArch64 under qemu; `tools/check-native.sh` runs it. Integer comparator transport, including
128-bit values, remains checked by compiled Order values.

`hirsel-rt/tests/io.rs` performs real disposable-filesystem interactions, records completions, consumes the same tape with SimIo and converts its requests/faults into interpreter capability calls. Every completion, exact stdout (including a split UTF-8 prefix), final tree and surviving grant matches. A second interpreter pass strictly consumes its canonical capability-event tape. Other cases reject missing/extra/reordered/forged events before effects; confinement and inode replacement tests retain accepted regular-file twins.

`exact-sum.hir` and `map-order.hir` are accepted source programs; interpreter execution produces `ok\n` and `74`, respectively. Compile/link parity remains open pending I/K/N physical-value glue, as recorded in `stdlib/native/README.md`. Standalone Rust/C probes establish kernel and adapter semantics, not completed backend integration.

`views.c` checks scalar/relative-position cursors over all 1093 official
Unicode16 input strings on both targets; x86-64 uses ASan/UBSan. It checks
slice offsets, empty-terminal positions and record offsets through the four
remaining Unicode exports. Grapheme segmentation is ordinary std source:
`hirsel-driver/tests/stdlib_graphemes.rs` executes every official boundary case
on full strings and nonzero-origin slices, while `hirsel-rc/tests/library_views.rs`
turns the vectors into compiled source loops on both native targets.
There is no core grapheme operation or runtime segmentation export.

`text.c` probes the three text exports with real slice offsets, UTF-8 success and
error credit transfer, reversed-slice
status and unwritten outputs. It runs 1000 rounds in both runtime modes, auditing
zero live blocks. `hirsel-rt/tests/text_native.rs` compiles and executes this
probe on both targets (x86-64 ASan/UBSan), checks compiled text/failure observations
under all 42 configurations, and proves generated drop glue releases a runtime
decimal result after each of 1000 calls in both modes and at O0/O2.

The revision-7 source transport cutover advances the declaration digest domains.
At that cutover, all 143 paths and ownership columns were unchanged; the 236
registry review declarations were compared byte-for-byte before replacing those
versioned hashes. Native classification retains the same exact-digest guard.

Wave 2 retires the bytes.replace and map.same_entries core rows. Both compile
from ordinary std source, and byte replacement has no runtime export. Source
conformance replaces the retired primitive replacement probes.

The four Io projections return inline Result[capability, GrantError], with
NotGranted as a nullary error. The 16-byte native record has a tag at offset
zero and payload at offset eight on both targets. FileError appends NotGranted
at ordinal two. RealIo and SimIo derive denial from their configured projections,
record it without an injected host fault, and cannot replay a forged handle.
The acquisition controls in hirsel-driver/tests/fixtures/grants cover granted
and explicit-empty simulation inputs; CAP-1-acquire-*-result-run exercises their
native matrix. Production admission owns applying ValidatedRun to these seams.
