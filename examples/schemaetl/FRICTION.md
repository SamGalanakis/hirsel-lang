# Friction ledger

Every source diagnostic below was an intended language rejection; no runtime/compiler
repair or language relaxation was made. Failed source drafts are external draft-01..07,
with exact check-01..05 logs under the lane's retained evidence directory.

1. Syntax: bare assignment match arm expected comma/newline; reserved `result`;
   attributes on a closure expression rejected. Use a block arm, `outcome`, and
   std string.compare. Cost none; ~10 active minutes, intended, schema-etl-30.
2. Name resolution: locals shadowing top-level declarations or root modules
   (`input`, `group`) rejected LANG-4. Rename helper declarations/local descriptors.
   Cost none; ~10 min, intended, same owner, check02.
3. Prefix inference: untyped None/empty seed cannot infer generic actuals.
   Annotate Option[Group], Result[Record,RowIssue], List[Row] before callbacks.
   Cost none; ~5 min, intended, check03.
4. Flow source preserving contract: copy/unique out of generic mut cursor rejected
   MODE-1/CLO-4; generic captured content must remain within its contract. Move each
   owned optional slot through replace, then wrap the ordinary result iterator with
   stream.from_results. Cost one bounded list of optional slots; ~15 min, intended.
5. Task crossing: copying owned config into task discarded uniqueness; String error
   aliased result failed MODE-4. Take config/input into worker, return unique String
   error payload. Cost bounded copies at admission plus small diagnostic copy;
   ~10 min, intended, check03/check04.
6. Scoped callback capture: mut state captured read-only cannot be written, and
   borrowed plan cannot escape into Flow callback. Copy owned field descriptors for
   output callback; materialize bounded Shared batch copy before mut sink work.
   Cost bounded extra batch/field copies; ~10 min, intended, check03/check04.
7. Schema issue bug in application: re-wrapping scalar error with invalid at
   max_issues=1 discarded original issue. Independent Python oracle caught exact
   bytes `row/invalid` versus `amount/type` in mixed-ref01 (258795 reference steps).
   Propagate original typed error and scalar field site. Fixed; ~10 min; same owner.
8. Buck packaging: runtime app paths missing from pool data; include actual six
   module/manifest/fixtures at compile time and rematerialize same package. The next
   build still lacked include dependencies until sync.py --if-stale regenerated
   its include graph. Cost temp package fixture copy; ~10 min, tooling, fixed.
9. Buck executor strips outer environment: requested mixed-only ran first 22 cases
   through bytes-65536 instead. Retain all 44 greens; use runner --env for future
   partitions. Cost none, ~5 min, tooling/fixed, shepherd guidance 643131.
10. Step bounds: bytes-65536,rows-1024,output-262144/262145 stop at 8M in reference and
    checked H (8 cells), no semantic observations. Vectors and bounds unchanged;
    both native backends pass their live datasets. No optimization/budget increase.
    Incomplete tooling; schema-etl-30, ~5 min, exact logs retained.
11. Native Pending entry: public Fixture supports file_reads but not application
    Args/files/mount snapshots. Current full-app witness confirms empty-Args usage exit. Owner reply
    643354 assigns native-harness-gaps-30. No adapter invented; obligations remain incomplete.
12. Initial asynchronous shell omitted wait: third test process ended without its
    EXIT marker. No test result claimed. Every later background command uses a
    guaranteed producer and `& wait`; setup/fixed, ~2 min.
13. Sealing: source/owned/generated stamps/config sealed before application readers;
    toolchain CLI/image/clang/lld/LLVM bytes sealed in app-check01/native-build01.
    Detailed archive/kit leaf inventory was late (toolchain-late-seal.json), labelled
    honestly; no semantic green repeated solely for improved sealing.

Owner disposition 643354 confirms native scripted adapter gap, now owned by
native-harness-gaps-30. Both backends executed the exact six-module entry in
schema_etl_30_native_scripted_entry_boundary_witness and returned usage exit 1
before input projection, as predicted. This is a current witness of the missing
Args/files/mount fixture path, not an application semantic pass. Owner explicitly
instructs landing useful app with native cancellation/replay/width cells incomplete.

## Final verification disposition

At base d49af62d, 38 workflow identities were frozen in the independent Python
oracle (the original 35 plus projection, optional-null and reorder). The original
35 expected vectors stayed byte-identical when the final three were added.

| Provider/engine | Completed | Incomplete |
| --- | ---: | --- |
| Scripted reference | 34 semantic passes | 4 Steps at 8M |
| Scripted checked H | 34 semantic passes | 4 Steps at 8M |
| Host files LLVM | 35 semantic passes | scripted provider matrix owned below |
| Host files Cranelift | 35 semantic passes | scripted provider matrix owned below |
| Immutable host tapes, four replay engines | 264 passes | 16 no completed observation |
| Production Ordered success/error, widths 1/2/4 | 12 passes | cancellation widths owned below |
| Ordered semantic event comparison | 12 matching vectors | none for these vectors |
| Every Ordered tape through four replay engines | 48 passes | cancellation tapes owned below |
| Changed input/config/extra-event controls | 24 refusals | none for these controls |

The four step-limited identities are bytes-65536, rows-1024, output-262144 and
output-262145. Both reference modes consumed the requested 8,000,000 steps with
no semantic observation. Both native recordings also have an incomplete reference/checked-H replay
disposition (16 cells): the current public replay API returns generic
`Runtime("replay produced no semantic observation")`. Its exact underlying cause
is not exposed and is not asserted to be Steps. Precise replay-outcome reporting
is owned by native-harness-gaps-30. Native replay passes. Bounds
were never raised and the datasets were not replaced. A passing tooling
classification test is not a semantic pass for these cells.

`native-harness-gaps-30` owns full-entry native scripted input/Args/mount fixtures,
including Pending/AfterWrite cancellation, sink faults, cancellation recording
and replay, and production Ordered cancellation at widths 1/2/4. The current
public Fixture defaults execute the real app entry with empty Args, returning
usage exit 1 before a file read. The exact two-backend witness and owner reply
643354 are retained externally. No adapter or surrogate entry was introduced.

Each of the 12 qualified live Ordered receipts has worker_phase=running,
effective=requested and pending=0 before completion, with logical jobs=4 and a
fixed four-CPU wrapper allowance. This proves capacity, not simultaneous CPU
execution. Every semantic event stays in its original order when compared across
widths/backends; no task IDs, errors or clocks are removed. All 82 recordings
(70 workflow tapes and 12 width tapes) replay with original input/output roots
unavailable. Every original tape is immutable. Recorded handles are audited for
close/commit and full write requests preserve data and summary bytes.

Five Rust test identities were added (three default, two explicit retained-tape
inspections). The original harness listed 436 tests; the final harness lists 441,
including ignored identities. The only registration edit is `mod schema_etl_30;`.
A repeated event-vector inspection added no coverage; it was accidentally queued
before noticing the prior combined selection had already executed both tests.
This does not replace any original receipt.

Evidence and exact failed drafts/logs/seals live outside the repository under
`/workspace/code/hirsel-briefs/schema-etl-30/`. No performance claim is made.
The archive/kit detailed leaf inventory is explicitly late; earlier seals cover
application/stdlib/compiler/image/generated graph/config boundaries. Original
passing semantic workflows were not repeated for better sealing or main movement.

The final report retains at most 32 JSON issue objects and counts every rejection.
Intermediate typed row statuses are bounded by the 1,024 admitted rows; they are
not an unbounded diagnostic store. Current ownership plumbing deep-copies each
Shared batch for the ordered sink and its output builder for the consuming Flow
fold. Cumulative copying can be quadratic in these small declared bounds; no
unsolicited optimization was added. Owner schema-etl-30, intended, approximately
15 active minutes across the cursor/Shared/builder decisions.
