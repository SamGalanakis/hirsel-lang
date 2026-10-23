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
application; fixed, no source/V disagreement or language-rule repair.

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
