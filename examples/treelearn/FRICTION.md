# treelearn friction

Qualification is recorded under `/workspace/code/hirsel-briefs/decision-tree-35/`.
No all-engine or physical-width claim follows from source checking.

## Intended API constraints

The std sort callback returns i32 and carries @noalloc; the initial draft's
boolean comparator was corrected by reading std@1/list.hir before checking.
Cost: no extra copies or runtime operations. Active time: 1 minute. Intended,
application owner decision-tree-35; no compiler defect claimed.

Flat tagged nodes and an owned bounded DFS stack implement persistent tree
ownership without a recursive training call stack. Generic Dataset[T]/Row[T]
scoring borrows data; each child partition owns its index list. Task results
are owned and parent publication is ordered. Serialization uses std JSON.

## Initial source admission (intended rules)

Original draft retained as `original-draft/*.hir` with exact diagnostics in
`check-1..4.log` in lane evidence. LANG-14 rejected `for result in ...`:
"result is reserved for the returned value in a postcondition". Renamed to
outcome; no runtime/copy cost; 1 active minute. LANG-1 rejected an inline
`@noalloc fn` expression; used a named annotated comparator, no runtime cost;
1 minute. LANG-1 rejected `loop { ... }`; this language uses while. Adapted to
bounded traversal; no new language rule or compiler bug claimed. Owner:
decision-tree-35; intended, closed.

LANG-4 also enforces no local/top-level shadowing and module-prefix precedence
(`input.data` resolved input as a module). Renamed input to specimen, local
label/counts to label_id/tallies and repeated value to item. Cost: none; 2
minutes; intended closed. std u64 helpers are exported as std@1/u64, not the
source filename u64_ops; fixed the call path, no runtime cost; intended closed.
json.parse consumes String, so the byte admission path uses string.from_bytes
and moves the result; no codec duplication, one ownership transfer; intended.

MODE-1 correctly rejected `copy counts` escaping a borrowed model into an owned
prediction. The explanation must outlive the model view: `unique counts`
copies at most four u64 counts per query. Active time 2 minutes; intended,
closed; owner decision-tree-35. Original diagnostic retained in check-4.log.
Prequalification check-5 had late source edits while its initial parsed source
was still being checked; it is retained as setup failure, never acceptance or
a complete immutable-closure claim.

MODE-2 rejected returning a unique Model after inserting `Leaf(copy initial,
...)`: an ordinary counted alias does not restore uniqueness when it dies.
Original response is check-6.log. Compute the label before transferring counts
with `take`, for root and child leaves. No additional copies or allocation;
1 active minute; intended closed. The first queued test action was canceled
before tests started because this known source error made execution pointless;
its EXIT 141 remains an unrun identity, not a test pass.

## Recovery boundary: owned recursive loaded models

The first classifier walked validated wire indices. The final loader assembles
an owned recursive Tree after validation; prediction borrows that immutable
tree and moves a single owned explanation builder through it. The retained
wire model allows exact re-serialization. Cost: at most 31 recursive nodes and
16 leaf count copies (at most four u64s each); depth remains at most four.

The initial splitter re-partitioned/scanned every candidate. The final splitter
ports sklearn's cumulative-count update over sorted observations, preserving
every original threshold/tie/partition output. It partitions owned indices
only after selecting the winner. `unique parts.left/right` copies each selected
partition into its work item, at most 64 indices per split; no shared mutable
index list enters a child. No compiler workaround claimed.

LANG-14 rejected the field name flat (reserved); renamed wire (zero cost,
1 minute). OWN-2 rejected `unique parent_counts` in a feature loop because a
unique operand moves; `copy parent_counts` retains it, with COW detachment of
at most four u64s per feature. Intended, closed, 1 minute. Original check-8/9
responses remain in lane evidence.

The first oracle generation froze model member order with version after
min_leaf. An intermediate draft changed that order before execution. Both
original and intermediate cases were retained; the original 40 cases have now
been restored byte-for-byte (including input and output strings), and the
writer follows the original frozen order. Earlier successful training output
cells are affected and must be recovered, not claimed for the final writer.

The initial reference campaign stopped at 1M steps on rows-64, batch-four,
bytes-65536 and nodes-31-jobs-{1,4}. Checked-H twins were unrun, not passed.
The five original witnesses/closures remain immutable. Recovery uses the same
complete bytes/caps after the actual app changes; no raised budget or smaller
replacement. Source checks alone do not qualify these original workflows.

## Native public observation projection

The first native toy cell passed H differential, LLVM live and both native
players of the LLVM tape. Its test then incorrectly unwrapped final_state
from replay_observations, whose replay enrichment intentionally has none.
This was a test assumption, not a compiler/runtime defect. The exact full-app
witness and raw tape remain in decision-tree-35-native-zyYkA4/toy; accepted
subcells are resume-marked and will not repeat. Test adaptation: compare the
public semantic replay projection; additionally check exact report WriteAll
bytes, the atomic output path, commit success/refusal, initial snapshot and
raw stderr on native runs/replays. Full final virtual file trees are checked
in reference/checked snapshot execution; replay consumes each recorded write
and commit request exactly. Cost: test-only inspection; 3 active minutes,
closed, application test owner decision-tree-35. No private fixture access.

Physical tests use a lane-local archive transport authorized by coordinator
message656880: one compressed/base64 tar per cell, encoded cap4MiB, digest and
length verified by a local extractor before extraction. This adds no shared
launcher surface. Missing/oversize/corrupt archives remain INCOMPLETE.

## Qualification inventory and default regression

The full simulation and native/replay campaigns remain explicit ignored
qualification tests. Selecting them with --ignored runs their entire original
inventory (or an explicit recovery selector), and any stopped/failed child
fails the campaign. No cap was raised, removed or converted to an expected
success. Default tests check the independent frozen oracle and a separately
named minimum-row full-main boundary (rows1/labels1/features1/depth0/min_leaf8,
CLI jobs1, unseen endpoint queries). This lower-bound regression supplements
the original upper-cap obligations; it gives them no qualification credit.

After the actual splitter/tree changes, 13 affected workflows passed both
interpreters. Rows64 and batch-four still stop at 1M reference steps. All five
previously unrun checked-H cap twins were attempted separately and also stopped
at 1M steps. The three reference stops in unchanged JSON parsing were preserved
without repeating them. These ten engine identities remain INCOMPLETE.

Attempt retention limitation: resuming the native toy cell reused its harness
stdout/stderr filenames before the old streams were copied. The original Buck
action log, captured error, source/tool closure and LLVM tape survive; the
original child libtest stdout is unavailable. The completed toy tree and resume
receipt have since been copied to an immutable evidence directory. This is an
application test bookkeeping defect (owner decision-tree-35), not a runtime
fault; future cells use separate roots/attempt copies. No app stream or tape is
substituted by the overwritten libtest stream.

## Native campaign boundary

Both native backends and cross-native replays complete the five original cap
workflows. Their reference/checked interpreter replays remain stopped (20 tape
player identities); native heap additionally stops on bytes65536 and the
31-node/four-job manifest. These are separate from the ten live interpreter
Stops. All raw tapes and per-engine receipts remain in native-manzeH.

The lowered native Heap adapter rejects any nonempty typed file-fault queue:
"native heap: file faults require reference/checked simulation or native
snapshot providers" (differential.rs569–571). The exact commit-denied full-main
witness is immutable in commit-denied-attempt1-immutable and routed to the
coordinator (message657647), with no local repair. Recovery records only this
exact known unsupported response as INCOMPLETE and continues missing native
snapshot cells; it never awards Heap credit. Reference/checked simulation of
commit refusal passed. Cost: test-only dispatch, 2 active minutes, open adapter
coverage owned by coordinator/native Heap provider; intended guard.

The early worker wrote passed.json even when incomplete.json existed; aggregate
receipts always rejected those cells. Future workers omit the passed marker
when incomplete. Earlier misleading markers are retained as bookkeeping
failures and do not override the incomplete per-engine receipts.

Scripted Ordered qualification is separately prepared for one selected
success/domain/Pending-cancel case at each backend/width. It sets only the
validated public run record's ordinary placement request, records full tapes,
withholds live files/scripts for native replay, and compares complete semantic
tapes across widths/backends (only admission time/placement metadata removed).
It gives no physical-capacity credit without the separate real-host controls.

Commit-refusal recovery completed both native live cells and all eight replay
players. Its known Heap adapter refusal stays INCOMPLETE. Default minimum-row
simulation remains a separately named pending regression until executed.

The fixed schema bounds compact output conservatively to28855 bytes (all
count components independently64, largest index/number spellings,31 nodes,
16 depth4 paths per job,4 jobs). An admitted32769-byte output is unreachable;
no fabricated over-cap output replaces a legal input boundary. The32KiB guard
remains defensive. Derivation retained as output-bound.json in lane evidence.

## Pool transport and minimum regression recovery

The first physical LLVM width1/success archive was refused at4764496 encoded
bytes against the unchanged4194304 cap. No runtime/capacity credit. Its raw
streams/tape were not exported; tape creation is unproved. Attempt2 used zstd
when available and stdlib LZMA otherwise, retained all program bytes inside the
cap, and extracted with3078 external tool inputs verified. It exposed the
standalone option-order defect: original grants-before-record argv produced
usage/exit2 and no recording. Both this producer repair and explicit Fixture
placement are owned by width-reruns-36; no local runtime patch or flag reorder.
The producer repair landed15260039, with pending importers resumed only after
a reader boundary. First-attempt missing artifacts remain explicit.

One combined pool regression action reached its unchanged RE600s timeout with
no completed receipts. Exact serial oracle45 then passed. The minimum fixture
was partitioned by adapter, preserving its original row/input/expected output,
1M steps,60CPU-s and2GiB. Setup now binds immutable SDK bytes by digest rather
than copying/recompressing the full SDK. Reference and checked adapters passed
separately with verified archives and3302 external input bindings. No accepted
inner cell was repeated to recover the failed aggregate.

Archive fallback now excludes already-external files from omission candidates
and binds qualified.image as a common compiler input. The prior fallback could
recompress the same98MiB image while omitting files already excluded from the
tar; that setup cost was unnecessary. Missing input bytes or oversized partial
capsules remain INCOMPLETE, not smaller accepted fixtures. Test-only cost;
owner decision-tree-35, fixed. The original600s action remains retained.
