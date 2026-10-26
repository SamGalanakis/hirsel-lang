# Spectral audit qualification

Original CLI, schema, input caps and Decimal intervals were frozen before
implementation under the lane's external evidence directory. Every original
case retains its identity. No compiler/runtime patches are owned by this app.

At launch, exact physical-width qualification waited for exact-cpu-rollout-34's client
landing confirmation. Scheduler deployment alone is not proof of admission.
Requested cells are LLVM/Cranelift × widths 1/2/4 × success/domain/cancel.
No Free mode, fake topology, unused timer, or source-only check qualifies them.

Disagreements and adaptations are recorded below with original source, exact
response, expected rule, ownership/copy/runtime cost, active minutes and owner.
Naming errors and intended ownership restrictions are not compiler bugs.

## Intended naming restriction (fixed; app owner)

Original `let mut result: List[U] = []` in Series.map. Source check:
`error[LANG-14]: result is reserved for the returned value in a postcondition; choose another name`
at samples.hir:22:11. Expected rule: LANG-14 reserves this identifier. Renamed
the local to mapped. No ownership, copy or runtime cost; <1 active minute.
Original full source is retained in the pre-first-test seal. This is not a
compiler defect.

## Match arm statements (fixed; app owner)

Original `Ok(value) => ordered[index] = Some(take value),` at analysis.hir:88.
Formatter/parser: `error[LANG-1]: expected comma or newline between entries`
at column 41. Assignment is a statement; used an explicit block arm, following
pnmtiles. No ownership, copy or runtime cost; <1 active minute. Intended syntax.

## Prefix types, names, callable modes and Args authority (fixed; app owner)

Source check response in check-2.log (full original source retained in pre-check-4):
`fn(sum: Complex, value: Complex)` supplied to `fn(take A,T)->A` gave LANG-7
(callback parameter 1 requires take); changed callbacks to take accumulators.
`let n = float.from_int(...)` and unsuffixed local float literals gave LANG-2
(generic arguments cannot be inferred / literal needs expected type); annotated
f64 locals. Local `table.values` gave LANG-4 (`table` names a module); renamed
the kernel parameter twiddles. Shadowed `value` and `index` gave LANG-4 (a local
cannot shadow another local); renamed the parser input and completion ordinal.
`arguments(args) uses {}` gave CAP-1 at arg operations (call authority exceeds
the declared uses restriction); declared uses {args}. These follow documented
prefix typing, naming, callable passing modes and capability rules. No copy or
runtime cost; about 4 active minutes.

## Generic map and output mutation contracts (fixed; app owner)

Original `unique Series(values: take mapped)` with unbounded U and an ordinary
callback result gave OWN-5: `Series(take mapped) cannot be copied, but a deep
copy is required`. The generic callback may return shared or noncopyable U; the
API now returns ordinary owned Series[U] without promising deep uniqueness. The
concrete f64 kernel only borrows that result. No extra copy/allocation; one
output list, no generalized unique result promise.

Original report.render passed its fresh output to ordinary mut helpers and
then returned unique Bytes. MODE-2: `bytes.from_list(take out) is not proved
unique`. Fixed by private summaries (b552ff23d839 / f71ba1242e00): the three private byte helpers need no preservation annotations or deep copy. The generic callback contract above remains declared. Original adaptation took about 2 active minutes.

## Rounded negative zero (fixed during review; app owner)

Original renderer used `abs(value) < 0.0000005`. Binary64's nearest 5e-7
is just below the exact decimal half boundary, so equality can still round to
zero. Changed the comparison to <= and added signed half-boundary controls
before any runtime test. No source/V disagreement or extra allocation; <1
active minute. Original below/above-boundary identities remain immutable.

## Evidence preparation timing (open; app and runner owners)

The initial seal preceded the first build action but iterated saved glob names
without re-evaluating them. The first test client was canceled while queued,
after std image/Rust harness builds, before any named app test or runtime cell.
The failed/partial generation and exit141 log are retained. Before runtime,
seal.py now re-evaluates live stamp files/probes/globs/outputs and traverses
symlink directories with ancestor-inode cycle protection. No acceptance cell
has been rerun for this correction. Source-check3 passes; source checking is
neither reachable V export nor execution.

Original output bound identities are 16384 accepted / 16385 refused. The
helper source spelling was added after implementation, before first runtime;
this is a late freeze, retained as such. It is an additional publication guard
control, not a full-app report or a substitute for original cap inputs.

The host evidence directory is not visible on hermetic pool workers. Runtime
qualification requires returned action artifacts (or an explicitly approved
transport) before sandbox cleanup. No runtime cell may silently use an
unretrievable temporary directory. Owner: agentlang-24 for returned-artifact
route, spectral-audit-35 for test adoption. No shared runner redesign is owned
by this lane.

## Returned evidence route (fixed; app owner, ruling 656743)

The owner confirmed the installed runner has no TEST_OUTPUTS_DIR and authorized
a lane-only stdout archive transport. Archive Drop retains tapes, exact app/std
source and seal metadata on both success and failure, using gzip/base64 with
a 4 MiB encoded cap, SHA-256 and compressed/encoded lengths between unique
markers. The invoking-host extractor checks both lengths and the digest before
extracting safe regular-file members. Tool bytes are retained in the host seal;
remote actual-tool digests join that closure. Human receipts stay outside the
archive block. No shared runner or launcher changed. About 8 active minutes of
plumbing investigation/adaptation; no app runtime cell preceded this route.

## Replay observation is not a snapshot (fixed; app test owner)

Original test `observed.final_state.as_ref().unwrap().stderr` after public
replay_observations panicked with `called Option::unwrap() on a None value`
at spectral_audit_35.rs:613. This API enriches semantic observations, without a
simulation snapshot. Compare raw native stderr and exact recorded PrintStderr
requests; successful interpreter replay validates their byte payloads. No app,
language or runtime change; about 3 active minutes. The original LLVM tape and
all completed simulation/native replay/control cells are retained, and recovery
menus skip them. No accepted unchanged app cell is re-executed for this repair.

The local extractor initially rejected the returned encoded length because
Buck prefixes each payload line with its timestamp. It now removes only that
exact framing prefix, then checks encoded/compressed lengths and SHA-256 before
base64 decoding/extraction. The original log is immutable; no test repeated for
this extractor fix. The first archive is 2396743 compressed / 3195660 encoded
bytes, within the fixed cap.

## Original full-main budget stops (open; coordinator routes std cost owners)

`length-32` is the original 32 complex samples (real=j-16,
imaginary=(j*7)%11-5), no mean removal, top8, band[0,32). Reference and checked H
stop exactly Steps1000000 at std@1/decimal_seq.hir:1557:3. LLVM full main and
both native replays completed; its reference replay stops at that same budget.
Original full source, complete input, stop receipts and immutable tape are
retained in workflows-1's returned archive. Routed to agentlang-24 in 657424;
no std repair, cap raise or smaller replacement. Runtime cost: no comparable
interpreter result, not a numerical failure or source/V disagreement.

`input-cap` is the exact original 32768-byte JSON manifest followed by spaces
(the one-over pair remains 32769). Both interpreters stop Steps1000000 at
std@1/json.hir:30:8. LLVM full main and both native replays complete; reference
replay stops at the same original budget. This is a distinct JSON whitespace
cost obligation, retained in workflows-3 and routed in 657665 with the exact
input bytes/hash. No cap raise, replacement or std patch. Original stopped
adapters are recorded separately and only missing adapters run in recovery.

The two original cap identities are retained in an explicit ignored
qualification test rather than making ordinary suites fail accidentally. That
test keeps the unchanged inputs and Steps1M assertions, reports INCOMPLETE and
fails when asked to qualify them. The ordinary workflow partitions are separate
original cases, not replacement controls. Native success never makes either
interpreter identity complete. About 4 active minutes of evidence handling;
no app source adaptation.

Recovery utility Rust annotation errors E0282 (unknown function-pointer result)
and E0603 (ToolError imported from private driver scope) were fixed with an
explicit Replay type using public hirsel_engine.ToolError. They are test-code
mistakes, with no app ownership/copy/runtime cost and no compiler defect.

Exact-CPU rollout is now confirmed at client 1da029061522de0222b8c29537b8dbc1a6fb34a5,
all four workers exact-v1. Capacity tests still need actual running control
replies; deployment is not app proof. The scripted width matrix changes only
the public validated run.json placement (cores1/2/4, shared commit, reorder off)
after Fixture linking. Args, mounts, scripts and every semantic tape event stay
bound. Its comparison removes only admitted_at and binding.placement, following
ordered_qualification.rs. Real-host capacity tapes compare only against their
own originals. No private runtime worker knob or topology override is used.
The capped wrapper now inherits the ordinary test action's HIRSEL_JOBS rather
than reducing an exact4 action's allowed affinity to one CPU.

The assigned std performance owner app-budget-reds-35 acknowledged both exact
witnesses in 657821. Its current scanner/Markdown/SHA/Chunk candidate does not
change decimal_seq or json. Both cost obligations remain separate followups;
no recovery is inferred from that scanner work.

## Recovery payload transport and standalone flag grammar (fixed; app test owner)

Original recovery-original-caps JSON was 223061 bytes in one --env argument.
The local subprocess failed before Buck launch with `OSError: [Errno 7]
Argument list too long`. No app cell started and no archive existed. Recovery
now carries gzip/base64 JSON in the environment, decoded by Python stdlib and
retained as exact recovery-input.json before the consumer. No tape byte is
changed; no system limit is raised. About 2 active minutes; transport-only cost.

Original ordered.py placed --hirsel-grants before observer flags. Physical
LLVM width1 success/domain/cancel returned status2, `spectral: usage`, no
control reply; the next width2 success action was interrupted after this
common cause was identified. All completed failure archives and the interrupted
action identity are retained. standalone.live_options consumes leading observer
flags only, then grants parsing snapshots every suffix word as application Args.
The owned script now uses pnm_tiles_30/qualify.py:12's documented order:
trace, record, grants, then unchanged app words. No runtime parser patch, cap
change, vector change, replay relaxation or control-assertion weakening. About
3 active minutes; no app ownership/copy/runtime cost.

## Fixture placement admission (historically UNRUN; fixed by dependency)

Shepherd658105 and owner658168/658210 identified the already owned producer
gap: launch_fixture selects HIRSEL_RT_FIXTURE_RUN, Selection.Harness has
requested=None, managed placement/control reads requested, and fixture_run
restores only Args. The scripted_ordered_streams importer requires actual
recorded placement, so it cannot earn width credit on this producer. At that boundary it had
never launched and remained explicitly ignored/UNRUN pending width-reruns-36's
landed repair. The completed continuation is recorded below. Routed in 658195. No duplicate runtime/Fixture patch. The real
ordered.py/CLI capacity route is independent. Owner will provide the repair SHA
for a missing-only importer continuation.

## Final missing-cell distinctions

Eleven physical cells passed with exact running/requested=effective/pending0
replies: LLVM all9; Cranelift width1 success/domain. Cranelift width1 cancel
returned0 before injection (`signal_delivered=false`), despite two running
control replies; this is neither a delivered-signal loss nor a capacity pass.
The injector had parked waiting for another file event after a running reply
but before io.signals installed its handler. It now continues the existing
bounded control/handler handshake while waiting for that projection. The
handler condition, delivered-signal requirement, expected canceled outcome,
cleanup, inputs and caps stay intact. Only this failed cell and the six unrun
Cranelift width2/4 cells may run. About 3 active minutes; no app/runtime change.

The old length32 checked-H replay now refuses program binding after the main
rebase, while all six app .hir files are byte-identical to the original archive.
The historical tape remains unchanged and incompatible; routed in 658827.
No fresh recording substitutes for that identity. New missing Cranelift live
cells use their own newly recorded tapes; historical LLVM results are retained.

The original aggregate unrounded oracle reference stopped Steps1M at
transform.hir:75:7 before a semantic observation. Its source/vector set and
budget remain unchanged and the aggregate stays explicitly INCOMPLETE/ignored.
Missing checked-H/native engines run separately on that same aggregate; native
math evidence never qualifies the stopped interpreter identity. No raised cap,
smaller vector, tolerance relaxation or replacement oracle. Owner: app lane.

Output-bound helper source had LANG-8 (`io.dir` FileError has no arm in
GrantError) and LANG-2 (bare generic Ok/Error comparison has no prefix type).
Handle the expected granted Dir locally and annotate the expected Result;
use existing list.filled for exactly16384/16385 bytes. No std error-arm change,
no input/cap change; <2 active minutes. Original diagnostics/source retained.

Producer repair 152600399b9ce9370bec9d2cda46bb9679d62c5d is landed. The original
observer-flags-after-grants argv is restored for only failed/unrun physical
cells, exercising that repaired parser. Earlier accepted capacity cells are
not repeated. The previously UNRUN scripted placement importer can now run on
the repaired Fixture producer with its original assertions intact.

## Qualified cells and additional mathematical controls

All18 physical capacity cells now pass: two native backends × requested widths
1/2/4 × success/domain/real delivered cancellation, with actual running,
requested=effective and pending0 replies, cleanup and own-tape offline replay.
Eleven retained greens were never repeated; only the failed/unrun seven ran
with the corrected injection handshake and repaired producer/parser.

All18 scripted width cells pass on producer15260039: complete semantic tapes
match across both backends and widths after removing only admission time and
placement provenance; actual recorded requested placement is asserted before
that projection. Pending reads and cancellation are real fixture events.

Both native engines pass the original aggregate unrounded Decimal oracle.
Reference and checked H retain the original aggregate Steps1M stop. Additional
per-vector controls use its exact frozen inputs and checks, selected by Args,
with the same Steps1M budget per run; they are additional evidence, never a
replacement or a green verdict for the original aggregate. The original kernel
bytes remain unchanged. No higher cap, smaller sample vector or tolerance
change. Output16384/16385 helper controls pass on reference and checked H.

All32 additional per-vector controls pass (16 original vectors × reference and
checked H), including exact original length32 inputs and dirty scratch reuse.
These qualify their own named identities only. Across 46 workflows (39 frozen
originals and seven additional fault/rounding cases), 180 full-main engine
cells completed; four original live interpreter cells remain Steps1M. Completed
non-stopped interpreter replays and native cross-backend replays are retained.
The original LLVM input-cap checked-H replay is still UNRUN; the original LLVM
length32 checked-H tape remains incompatible at binding.program, rather than
being replaced by a fresh recording. The corresponding original reference
replays retain their recorded Steps1M stops. Missing Cranelift interpreter
replays also retain their original cap stops.

No compiler/runtime/std code changed in this lane. Source-check corrections
above were legal application or test adaptations, with no observed source/V
disagreement. The final CLI harness has 587 tests: 12 declarations from this
lane, including three explicit ignored qualification tests; the starting tree
had 565 and unrelated incoming main changes added ten. Individual engine,
replay and matrix subcells are counted separately from Rust test declarations.
