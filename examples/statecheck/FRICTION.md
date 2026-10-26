# Statecheck qualification and friction

Schema and 46 original oracle cases frozen before implementation. Qualification
is pending. No engine or physical-width cell is claimed by a source check.

Intended ownership adaptation: generic visitors are dictionaries of pure
callbacks. Safety/deadlock dictionaries own explicit unique copies of at most
64 policy indices (and at most 128 source indices); nested scoped callbacks
borrow each State, return only bool, and retain no graph view. Parent, lowlink
and SCC label buffers stay unique. Workers share the immutable graph batch and
return owned fallible report bytes. Owner: state-model-checker-34. Active time:
implementation, not a checker workaround. No language rule change requested.

LANG-14 intended naming rule (1 active minute): original `let mut result =
unique Search(...)` in reach.bfs was refused: "`result` is reserved for the
returned value in a postcondition; choose another name". Renamed to `search`;
zero ownership/copy/runtime cost. Original source and exact response retained
in generation-01 and state-model-checker-34-simulation-01.log. Not a compiler bug.
Owner: application; fixed.

LANG-4 and MODE-2 intended contracts (3 active minutes): the original main
rebound `data`; the batch parameter/callback reused `input` (also a std module
name). Renamed those bindings without runtime cost. Original `witness ->
Result[Option[Lasso], Problem]` did not promise a unique returned payload, so
MODE-2 correctly refused storing its lasso in `unique Verdict`. Strengthened the
producer result to `Result[unique Option[Lasso], Problem]`, moving fresh traces
without copying. Original generation-03 and exact check-01.log retained. Owner:
application. Private helper case fixed by private summaries (b552ff23d839 / f71ba1242e00); current exported witness retains its declared result contract.

Application schema adaptation (4 active minutes): original derived Batch
decoder accepted `{"version":1,"models":[],"extra":0}` then returned `models`,
while the frozen closed schema requires `schema`. std derive intentionally
ignores unknown object members; added object-cardinality guards for State, Edge,
Graph and Batch around the existing std derivation (required fields still use
std lookup). Cost: one object view and cardinality comparison per object, no
codec or std change. Original source generation-07 and simulation-02.log retained.
Owner: application; fixed. This is not a source/V disagreement.

Original interpreter cap cells: cap-states, cap-edges and over-edges each stopped
with Steps limit=1000000 on both reference and checked H. These six original
full-main obligations remain INCOMPLETE; source/input/oracle/assertions/budgets
are retained. No smaller control replaces them. Owner: application qualification.

Selection error (2 active minutes, harness; fixed): Buck does not forward shell
STATECHECK_CASE into its executor. The intended selective simulation03 actually
repeated 60 previously passing and 6 stopped identities, then completed the
remaining cases. Both attempts remain retained (158 attempt receipts, 92 unique
case/engine identities). Final results: 84 passes and 8 original Steps1M stops,
including cap-input on both engines. Future selections use executor
`-- --env STATECHECK_CASE=...`; no accounting rerun was performed. Original
failed actions remain failures. Owner: application qualification.

Harness API correction (3 active minutes; fixed): `replay_observations` has
`final_state: None` for host replay. The original native01 run passed LLVM and
Cranelift differential on networkx-scc-0, recorded LLVM, passed two native
cross-replays and one reference replay, then the test unwrapped that absent
snapshot. Removed only the invalid snapshot assumption. Native output files
are checked by exact atomic create/write-all/commit requests and successful
completions in each immutable tape, while simulation compares the complete
final tree and stderr. Host replays validate complete request consumption and
semantic observations; they never mutate host files. Added per-replay receipts
to resume the missing checked replay and subsequent cells. Original native01
source/response/tape retained. No compiler/runtime patch. Owner: application.

Physical capacity: coordinator explicitly confirmed exact CPU deployment is
unavailable (kiln PR86, build-infra PR94, Hirsel client draft PR1). Each combination
of LLVM/Cranelift × success/domain/cancel × requested width 1/2/4 is individually
INCOMPLETE, unstarted, owned by exact-CPU deploy (@agentlang-24):

| Backend | Outcome | W1 | W2 | W4 |
|---|---|---|---|---|
| LLVM | success | INCOMPLETE | INCOMPLETE | INCOMPLETE |
| LLVM | domain | INCOMPLETE | INCOMPLETE | INCOMPLETE |
| LLVM | cancel | INCOMPLETE | INCOMPLETE | INCOMPLETE |
| Cranelift | success | INCOMPLETE | INCOMPLETE | INCOMPLETE |
| Cranelift | domain | INCOMPLETE | INCOMPLETE | INCOMPLETE |
| Cranelift | cancel | INCOMPLETE | INCOMPLETE | INCOMPLETE |

After supported deployment, the coordinator routes these exact reruns from an
executor with the evidence destination reachable (no topology overrides):

```sh
.buck2/bin/buck2 test -c hirsel.tests=host //:hirsel-cli-test-cli-shard-1 -- --test-arg=state_model_checker_34_ordered_llvm --test-arg=--ignored --test-arg=--nocapture
.buck2/bin/buck2 test -c hirsel.tests=host //:hirsel-cli-test-cli-shard-1 -- --test-arg=state_model_checker_34_ordered_cranelift --test-arg=--ignored --test-arg=--nocapture
```

These tests require the exact running control reply (effective=requested,
pending=0), actual cancellation and identical full scripted semantic tapes
across widths, except documented admission/placement metadata. Free is off.

Native original cells on the pre-fixture-repair runtime: 92 differential backend
agreements; 92 immutable recordings; 184 cross-backend replays and 168 interpreter
replays passed; 16 original oversized interpreter replay identities stopped at
Steps1M. Binding/request/missing-event/extra-event controls passed on all four
engines for both backend tapes (32 controls). 52 raw live stderr cells exposed
the native-file-faults-33 omission (recorded stderr was correct); these original
witnesses remain archived. Native02 hit the unchanged 10-minute action timeout
with 64 agreements and 63 completed groups. Native03 recovered only unfinished
groups using the executor environment. No cap increase. All 92 original native
models, including cap/one-over vectors, completed; no source/V disagreement.

The fixture repair 593792b7/c6ebee19 is now on main. New consumers require exact
raw stderr and the launcher's public nonempty refusal record; the old local
refusal-record fallback was removed. Historical tapes keep their original runtime
identity and are not retaped under a new identity. Supplementary full-main fault
fixtures use only the documented public FileFaults: open, read, atomic create,
partial write-all and commit errors. Their original output target must survive,
and all temporary handles must be cleaned up. Qualification pending below.

Evidence setup limitation: initial prepare generation preceded the first complete
source seal. Generation-01 records that partial setup seal honestly. Subsequent
application consumers retain source/build-input inventories and actual compiler,
std image, runtime/linker/helper bytes before application execution. This is not
an all-green claim for a retrospectively sealed setup action. Owner: lane process.

Final focused qualification (after the shared fixture repair): all 52 affected
raw live stderr cells passed with exact status/output/stderr. Five file-fault
workflows produced 10 reference/checked passes and 10 native agreements, 10 new
immutable tapes and 40 four-engine replays; the complete simulated output tree
preserved its original target, with no leaked temporary. Both native backends
passed the report byte-cap and trace-cap exact/one-over helper test. The affected
refusal paths were requalified on two new self-loop tapes, with 32 four-engine
binding/request/missing-event/extra-event controls and eight successful replays.
Old tapes and original omission witnesses remain retained, not replaced.

The first post-repair std-image reader lost its remote connection during the
coordinator's authorized scheduler restart: zero test bodies ran (action digest
53402532acfe9d49a756f655bd54154bc5db2c70e57787231f371b2c58639246:142).
An already-launched retry and the remaining affected consumers passed after
ordinary-pool recovery. No local fallback or resource-cap increase. Original
infrastructure failure and distinct successful retry are both archived.

Example grant adaptation (1 active minute; intended rule): original host path
`examples/statecheck/fixtures` would be resolved beneath the grants file's own
directory. Corrected it to `fixtures`, following main's documented grants-file
base. No app runtime/copy cost; source-only configuration correction, no compiler
bug. The output host `.` intentionally places the default report next to the
manifest. Owner: application; fixed.

Open acceptance cells: 8 original direct interpreter Steps1M cells (cap-states,
cap-edges, over-edges, cap-input, each reference/checked); 16 corresponding
interpreter replay cells (four cases × two native tapes × two interpreters),
owned by application qualification. Original complete fixtures, source sites and
fixed budgets remain in support cases and archived receipts. Also the 18
physical Ordered capacity cells listed above, owned by exact-CPU deployment.
No native fixture fault, stderr or refusal obligation remains owner-blocked.
Five active Rust tests and two ignored deployment tests were added. No std,
language rule or compiler/runtime implementation was changed by this lane.
