# Propkit friction and verification

Application base: `ff3886a3b2e1dec2912efca90e4d995fa38f070a`. This application lane changed no compiler,
runtime or std code. The six modules contain 1,279 formatted Hirsel
lines: main205, strategy183, shrink133, runner228, replay282, properties248.

## Verification disposition

| Identity | Disposition |
|---|---|
| 30 complete CLI workflows × reference / checked-H | 60 PASS; semantic-06.log |
| Same workflows × LLVM / Cranelift | 60 PASS; native-01.log |
| Those 60 recordings × reference / checked-H / LLVM / Cranelift replay | 240 PASS; immutable-byte and event-consumption assertions |
| Independent seed, convergence, tree rollback, bounds and generic map/filter vectors | Four engines PASS; contracts-02.log |
| Production Ordered success / property error / cancellation / file replay, LLVM widths1/2 | Eight completed live/native-replay prefixes, inferred from original loop position; exact readiness/tapes unretained, therefore archival/public-replay obligations INCOMPLETE |
| First production width4 | INCOMPLETE capacity: running, requested4, effective2, pending0; missing-01.log |
| Missing-only local production recovery: LLVM width4 and Cranelift widths1/2/4 | 16 live PASS, 32 native replays PASS, running/effectiveN/pending0 receipts and raw tapes retained; production-02/ |
| Recovered tapes through public reference / checked-H adapters | 32 PASS; production-02-replay.log |
| Complete semantic tape comparison across Cranelift widths1/2/4 | PASS; only binding admission timestamp and placement removed, no event sorting or task/error/clock normalization |
| Changed Args / config refusal | Eight engine cells PASS; production-02-replay.log |
| Extra suffix refusal / Steps1 controls | Suffix reference/H cells passed in the original audit; two native suffix cells and two previously unrun Steps1 checks passed in recovery. The old fixed engines=4 label overstated recovery alone; production-02-replay.log / controls-02.log. Original suffix-output assertion stays red. |

Small workflows use the unchanged 1M adapter policy. Byte-cap and cases64 fixtures
predeclare 10M; cases64 consumed683980 steps. The supplemental convergence fixture
predeclares10M and consumed1753393. Step accounting remains outside observations
and tapes. Every native application process has CPU60s/RSS2GiB bounds; build setup
has normal lane limits. No Free or test-only scheduling was enabled.

Native-01's remote scratch tapes were not exported. Assertions exercised all
four replay adapters, but raw-tape archival remains INCOMPLETE for those exact
identities. Production-02 retains representative actual tapes without relabelling
or rerunning the earlier cells. The first remote production helper also lacked
per-cell flushing; its completed LLVM widths1/2 prefix cannot be publicly replayed
without its lost tapes. These gaps remain explicit; this is not all-green archive
qualification. The permanent helper now journals every cell and accepts exact
missing-cell selection. Production tests are ignored in routine pool runs because
width4 requires a local four-CPU allowance. No simultaneous hardware use is claimed.

Evidence, failed drafts, immutable witnesses, tool/source hashes and logs are
external under `/workspace/code/hirsel-briefs/property-kit-31/`.

## Contracts, workarounds and owners

| Attempt and actual diagnostic/behavior | Expected contract and application disposition | Cost / active effort / owner / witness |
|---|---|---|
| Reserved `result`/`before`, inline `? fn`, JSON-like map literals and local shadowing; LANG-14/LANG-1/LANG-4 | Intended rejections. Use distinct names, named error conversions and `map.insert`. Ordinary application plumbing. | No special copy; ~20min; app; check-01..05 and draft copies |
| Named callable passed to a unique callback position; MODE-2. Generic map inferred `Strategy[unique Sample]`; LANG-2/MODE-2 | Fresh owning function literals and an explicit `Strategy[Sample]` result annotation preserve exact nested contracts. Intended. | Small closure environments; ~10min; app; check-05 |
| Task return carrying String-bearing record/errors; MODE-4 | Explicit `Result[unique Option[Record], unique ReplayError]` owns both paths. Inert/copyable bounds restrict strategy/shrink seams. Intended. | Bounded unique candidates; ~10min; app; check-05..09 |
| Weak `number()->Result[u64,ReplayError]` propagated into unique error return; source accepted, V constructor-field/sendable-receipt refusal | Under prefix contracts this should reject MODE-2. Strengthen helper's error header. V remains correct; source-checker repair belongs to generic-loop-matrix-31. | No extra copy/allocation; ~15min; owner generic-loop-matrix-31; witnesses/decode.hir, decode-2.log; original v-app-07 |
| Generic `unique state.frontier[0]` then `replace(mut state.current, take candidate)`; source accepted, V replacement/BorrowedMove/InvalidResultSource | A generic unique copy retains Local sources and cannot enter core replace’s fixed Global receiver. Move/remove the owned candidate instead. The source rejection was fixed in 90c7bd12; V and the lifetime contract were already correct. | Avoids candidate copy; ~10min; owner replace-unique-copy-31, FIXED by 90c7bd122097196183bc1f3894640aa2cdb54e74; witnesses/replace-min.hir and log, semantic-03 |
| Mutable generic tree captured in scoped reductions callback; source accepted, V closure-sendability | Legal CLO-1 borrow. V's symbolic receipt used the aliased physical `Tree[T]` row instead of its current unique content. Scoped receipt validation now replays actual operands, retaining exact generic dependencies and rejecting forged crossing, capture-use and locality claims. The complete original four-module fixture and inline/helper value and cleanup twins are covered by `propkit_scoped_receipt_38`. | The shipped reductions helper remains a library seam: this lane qualifies the original inline minimize entry, not the six-module app's historical replay/Ordered identities. No remaining source/V workaround is required; original witness and failed run.log remain historical evidence under property-kit-31. |
| Stored `Source[T].draw` returning `Result[unique T,...]`; source accepted, V call-result refusal | Public borrowed generic `strategy.draw` preserves the callback result contract. The helper positive and full app pass; no V relaxation. | One ordinary helper call; ~10min; owner generic-loop-matrix-31; immutable call-result-v2-646701.hir/.hir.json/.log and helper twin |
| Mutable report captured from outside task scope; CLO-1 | Construct report inside the scope. Intended borrowed-capture restriction. Chosen unique case is recovered via `Shared.thaw`, with deterministic redraw fallback. | No unbounded state; ~5min; app; draft-07 / check-08 |

The library frontiers own each candidate and rollback value. Concrete list copies
are bounded by eight elements, and property evaluations share the 64-candidate
budget. `Shared` transports inert owning callable environments across tasks; it
does not share mutable counted data. Replay's 64KiB byte cap precedes std DOM
parsing; the eight-element check precedes building the application's value list.
DOM parsing has its own bounded-by-input allocation overhead. No duplicate JSON,
PRNG, sorting, reversal or codec implementation was added.

## Setup and evidence limitations

The matrix was written after the first draft, before builds: a late preimplementation
seal. Early source seals were subsets: they omitted evaluated `generated.BUILD_INPUTS`
and live stamp probes/globs; Python `glob(dir/**)` also omitted file members. A full
union was captured during check-06 and matched at exit; later source/graph/tool
seals are retained before and after readers. No green cell was rerun to strengthen
provenance. Native-01's std-image artifact changed during Buck materialization from
prelaunch7d0d010c to exitcd6f117a while sources and graph were unchanged; its exact
artifact receipt is late, bound to buildIDf5b899be-cb37-47a8-8194-14498e045c13.

Check-06 accidentally applied the runtime CPU60 cap to compiler setup and exited152;
it is a setup stop with no observation. Check-07 used the normal build allowance.
Explicit std-image preparation reduced subsequent setup cost. A Rust harness
panic-helper draft hit E0271 and ran zero tests; a generic helper corrected it.
A supplemental vector draft shadowed its sample name in closures and was rejected
LANG-4 before entry; contracts-02 fixed only the support names. Source checks alone
were never counted as execution. Full `check_for_execution` exposed the specialized
V refusals above.

The original suffix-refusal test incorrectly expected empty stdout. Extra input at
tape end is discovered only after the valid request prefix has executed; the
runtime may have re-emitted the complete recorded stdout before refusing unconsumed
input with exit3. The corrected test requires exactly that recorded prefix and no
file effects. Input/config binding refusals still require empty stdout. Original
red and missing-only recovery receipts are distinct; no live recording was repeated.

## Reference adaptations

Proptest70cf37d4 traits.rs580–621 and num.rs280–345,995–1069 supply the
simplify/complicate protocol and signed convergence vectors; the MIT notice is
retained with the upstream source copyrights. Hirsel history8530c5a0
history.rs333–358 supplies deletion/restart structure. Typed outcomes/errors
replace panic unwinding, owned bounded frontiers replace unbounded value trees,
and std Xoshiro256++ plus per-index seed splitting replaces upstream RNG state.
These are semantic/ownership/budget adaptations, not seed compatibility or a
performance claim. No optimizer or throughput work was done.
