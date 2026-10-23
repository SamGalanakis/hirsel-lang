# Flow audit 36 friction and costs

No compiler, runtime, standard library or language-rule changes are owned here.
Exact new source/V/runtime faults go to the coordinator; existing width recovery
belongs to width-reruns-36. Acceptance outcomes are recorded individually outside
the repository under `/workspace/code/hirsel-briefs/flow-audit-36/`.

| Original / expected rule | Adaptation and cost | Response, owner, disposition |
|---|---|---|
| Initial setup mkdir used the shared cwd | Moved the two new empty app/support directories into the owned checkout before authoring/consumers; original eight untracked paths confirmed intact | Setup correction only; flow-audit-36, fixed, 2 active minutes |
| NetworkX mutable dict residual | Unique nested dense 16×16 i64 matrices for capacity and flow; 512 scalar cells (4096 payload bytes) per scenario; paired directions share net state | Intended bounded representation; flow-audit-36, 5 minutes |
| Callback accumulator may contain ownership | Generic borrowed graph fold restricts accumulator to plain, capacities remain generic; no hidden collection clone | Intended interface; flow-audit-36, 2 minutes |
| Shared topology/capacity scenario inputs | One topology owner frozen into Shared; scenario capacities copied into each child, at most 64 u64 values/512 bytes per child; task result transferred into add-order slots | Intended isolated task ownership; flow-audit-36, 5 minutes |
| BFS frontier selection | Explicit `copy left/right` shallow immutable frontier view, at most 16 ordinals; previous frontier lives through iteration, fresh next list moves afterward | Intended borrow lifetime; flow-audit-36, 2 minutes |
| Complete encoded report validation before atomic create | `unique encoded` copies at most 32768 bytes to verifier; derive decoding duplicates bounded typed arrays and names; no solver rerun | Intended validation cost; flow-audit-36, 3 minutes |
| NetworkX flow may be signed and antiparallel | Bounds proven before allocation: 64×1,000,000=64,000,000 aggregate; pair residual at most 2,000,000. Truncate only after range proof, i64 reverse subtraction deliberately negative | Intended exact arithmetic; flow-audit-36, 4 minutes |
| Frozen commit-failure fixture spells permission_denied | Public Rust fixture adapter maps that frozen spelling to FileFaultError::PermissionDenied; preserves original vector bytes | Fixture adapter only; flow-audit-36, fixed, 1 minute |

Qualification is pending until per-cell receipts establish completion. A source
check, reachable V export, zero-test action or Steps stop is never full execution.

Harness compile correction: copied tape control local `expected` shadowed the
outer Observation; Rust E0609 before any tests. Renamed refusal_expected.
Owner flow-audit-36; fixed, 1 minute, no production/runtime workaround.

Host routing: Buck host exact request 4 refused before libtest with affinity width
32. No app cell ran. Use supported pool request and export raw tapes/receipts,
fixture closure manifests and sources through retained stdout before sandbox
cleanup. Binaries are sealed by SHA and matched to the retained local compiler
input closure; any absent matching blob remains a partial seal, not a full seal.
Owner flow-audit-36; intended routing, 3 active minutes.

Source diagnostic LANG-14: `let mut result = take initial` uses the reserved
postcondition name. Renamed the fold accumulator to accumulated; intended naming
rule, no compiler bug. The solver owns its copied scenario name (`unique name`,
at most 128 bytes), since a borrowed String cannot escape in a global witness.
Owner flow-audit-36; fixed, 2 active minutes.

CLI check original source-naming closure: MODE-2 rejected returning std list.filled
as unique reached and json.parse's shared DOM as unique; MODE-2 rejected request
names copied into a unique task input; OWN-2 rejected unique encoded then reuse;
CLO-3 rejected a borrowed counted command String in Options; LANG-4 requested
derive helpers in certificate's owner module. These are intended rules. Build
reached flags fresh (16 booleans), keep DOM shared, own each copied request name
(<=128 bytes), shallow-copy encoded bytes into verification (no byte clone),
store the CLI command as a plain bool, and put public witness derive helpers in
certificate. Owner flow-audit-36; fixed, 5 active minutes. This supersedes the
initial expectation that complete report verification needed a 32 KiB deep copy.

The first ownership edit missed model.parse's formatter-collapsed one-line return
type, so check2 retained the original MODE-2 DOM error alone; corrected that exact
signature. Added bounded borrowed name preflight before derive allocation.
Owner flow-audit-36; fixed, 2 active minutes, no new runtime adaptation.

Preflight helper originally named `names` conflicted with ordinal's borrowed
parameter `names`: LANG-4 local cannot shadow a top-level value. Renamed helper
to preflight_names. Intended naming rule; owner flow-audit-36, fixed, 1 minute.
The already queued classic selector failed at source load, so it earns no runtime
credit; subsequent selectors are gated on a clean source check.

Original full-main classic reference passed (214847 steps), while checked H trapped
at residual.augment:81, the second nested mutation, after 101068 steps:
`allocation inside @noalloc (MEM-5): projected mutation may detach shared or arena
storage`. Original source and exact observation/receipt are retained in archive
simulation-EEUR08 outside the repo. Source/V accepted `mut r: unique Residual`.
Minimal original is tests/support/flow_audit_36/projection-original.hir; adapted
companion omits @noalloc. Remove only augment's annotation locally, retaining
unique residual ownership and paired signed updates. No algorithm/flow change;
allocation is now permitted, so a detach may cost a row clone (<=16 i64 /128
bytes per update) if runtime storage is shared. Route original to coordinator,
no competing interpreter patch. Owner coordinator; open, 5 active minutes.

The minimal reproducer's first fixture requested 100000 steps and the public
application API refused before entry: `application step bound 100000 must be at
least 1000000`. Retain projection-VpLEsI as setup-only, then retry under the API's
supported 1M minimum; no app vector or exhausted cap is replaced or increased.
Owner flow-audit-36; fixed fixture admission, 1 active minute.

Additional bounded ownership costs: constructing unique Search from std filled
predecessor/successor lists may copy two <=16-option arrays (<=512 payload bytes
per BFS); unique Residual construction may copy two <=16×16 i64 matrices (<=4096
payload bytes per scenario). Every completed report/candidate is validated
independently; accepted reference outcomes are reused as explicit native expected
data rather than running those accepted reference cells again. No native/replay
credit is inferred from that reuse. Owner flow-audit-36; intended representation.

The 1M small projection controls both passed (projection-c3DG4t); they are bounded
controls, not a reproduction of the full-main disagreement. Keep the full original
residual module in tests/support/flow_audit_36/residual-original.hir. The explicit
FLOW_AUDIT_36_ORIGINAL_NOALLOC selector installs those exact original bytes before
sealing and loading the real package. Run classic/checked-h with that selector to
reproduce the retained full-main red. Runtime owner h-projection-37 requested the
fixture; coordinator delivery ID660213. No assertion was relaxed.

Original full-main evaluator bounds: vertices-arcs-cap, overrides-cap,
overrides-over, input-cap and report-cap each stopped at exactly Steps1M in both
reference and checked H. Original per-cell receipts and complete source/input
closures are retained in cK9GAu, GFEE5Z, QscBg8 and 3SD96d archives. Never count
these ten executions as completed. Keep exact caps; no smaller companion or
higher step allowance replaces them. Native qualification may use independently
validated deterministic expected files, while interpreter replay Stops remain
red and explicitly incomplete. Owner coordinator/app qualification; open.

Replay bookkeeping initially cast the public replay function pointers with an
inferred Result error placeholder, causing Rust E0282 before Ordered libtest
entry. Use the ordinary inferred function array plus enumerate, preserving exact
ToolingLimit error assertions. Ordered setup-only failure is retained; zero app
cells ran. Owner flow-audit-36; fixed, 1 active minute.

Ordered original 18-cell run retained in ordered-alZXJp: five exact capacity
receipts passed and thirteen remained INCOMPLETE. All eighteen exact semantic
outcomes, native cross replays and normalized scripted tapes agreed. App fixture
root: run_capped.py copied a single-CPU default; tools/capped's documented sandbox
fallback restricted the inherited exact4 allocation to one CPU. Add explicit
Ordered-only --inherit-exact-cpus mode: assert inherited sched_getaffinity has
exactly4 CPUs after the supported launcher assertion, then preserve those four
through the capped helper. Never query host CPU count, fabricate grant/topology,
or change CPU60/2GiB limits. Ordinary native launches retain their original mode.
Recover only thirteen missing live identities; link accepted counterparts solely
as players for new tapes. Owner flow-audit-36 fixture; fixed, 4 active minutes.
Exact archive routed to width-reruns-36 (delivery661304); no shared runtime repair.

Closed nested-projection root: h-projection-37 landed
`a8770e931c6674cc9f71129559211e5a11ae93e1`. Checked native list.push had omitted
actual transferred child ownership subtrees; the owner repaired list edit witness
transport without changing source/V rules or guards. Original full Flow fixture
passed reference/checked at214847 steps/value2000, LLVM/Cranelift and8 cross-engine
replays in the owner's retained simulation-d4hTyx/native-SZP1qc archives. All five
other source modules are byte-identical to ours. Restore the exact original
residual module, including @noalloc; the allocation-permitting adaptation and its
original green receipts remain historical evidence. No projected detach is now
permitted in augment. Owner h-projection-37; fixed.

Final coverage additions keep the original43 inputs unchanged: logical-jobs-1/2
run the same genuine four-scenario batch through the CLI grouping branches, with
new identities and unchanged bounds. Publish the independently validated five
cap expectations as permanent test fixtures so native cap tests need no external
per-session baseline. Neither these expected files nor a passing native result
turn original evaluator/host replay Stops green. Named finite partitions are in
tests/support/flow_audit_36/partitions.json.

Ordered recovery: after preserving actual inherited exact4 allocation, twelve of
thirteen missing cells qualified. Combined17/18 capacity receipts pass. LLVM4
domain remains INCOMPLETE: the short child finished between startup/draining
control replies; no unchanged lucky retry or weaker predicate. All31 original
and recovery semantic outcomes and62 native replays agree; scripted semantic
tapes match across both backends/all widths after only documented binding
admission/placement metadata removal. Owner coordinator/control observation (width-reruns-36 owns older apps only);
original gaps and the remaining identity retained.

Evidence transport originally archived closure manifests while excluding tool
blobs. Static tools and all eight historical harness binaries were recovered by
exact recorded SHA256 from materialized artifacts/immutable CAS, with a late
supplement explicitly recorded; original pre-consumer partial seals are not
rewritten as complete. Optional export retains exact worker Python bytes on a
new fixture run. No accepted application cell is repeated to improve capture.

The first optional Python export retained /usr/bin/python3 as a symlink, not the
resolved interpreter bytes. Correct the archival path to resolve the symlink and
export it on the next unrun native logical-jobs fixture. Do not repeat accepted
reference/checked cells for evidence capture. Owner flow-audit-36; fixed helper,
1 active minute; original transport boundary remains recorded.

Final application qualification (fixed original limits): 45 input identities
(original32 audit/workflow +11 verify +2 logical-jobs). Direct reference/checked
qualification is80 passed/10 Steps1M; the earlier checked classic MEM-5 red is
retained separately. Native live90/90 and native replay180/180 passed;
interpreter replay160 passed/20 Steps1M. Four binding/request/missing/extra tape
refusal controls passed on all four replay engines (16 negatives). Six focused
Rust tests are registered, including the explicit ignored Ordered matrix; the
cap-limited aggregate wrappers remain failed, never reported as six green tests.
No completed unchanged app body was repeated for main movement or capture.

Both new logical-job inputs pass reference/checked at850799 (jobs1) and850549
(jobs2) steps, plus both native backends and their cross-engine replays. Reports
equal jobs4. Final native export retains the resolved worker Python bytes,
SHA256359ca926ba4596147d7d558c1e0cbf6a5784f5475d7e8bf0a55d9d7c4fa7106c;
older manifests match this supplement. Historical partial capture stays partial.

app-steps-37 supplied retained candidate outcomes for original input-cap315726,
report-cap181455 and overrides-over532914 in both interpreters. Independent
validation of exact inputs, outputs, full file maps and certificates passed
without rerunning bodies. These are candidate-revision results pending landed
std identity/closure proof, not a rewrite of ten original Stops or old tapes.
Vertices-arcs-cap and overrides-cap remain over budget even on that candidate.
Owner app-steps-37 for std cost; coordinator for old tape/control recovery.
