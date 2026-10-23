# Route planner implementation and qualification friction

The initial frozen interface and all 43 original input/oracle vectors live in
`/workspace/code/hirsel-briefs/route-planner-33/`. Qualification is tracked there
per cell; an unrun cell is incomplete.

No language or runtime change is proposed. NetworkX's arbitrary integers become
checked u64 with a typed per-search overflow result; stable input ordinals and
insertion serial replace arbitrary node comparison. Single predecessors replace
all equal-cost predecessors, preventing zero-cycle reconstruction. The bounded
array edge scan costs at most 64*256 edge inspections per query search. Shared
snapshots require one deep copy at batch admission; owned child results and
ordered parent slots avoid shared mutable output.

| Minimal original / response | Expected rule | Adaptation and cost | Active minutes | Disposition / owner |
|---|---|---|---|---|
| Exact CPU request is absent on fetched main (platforms.bzl has only focus/pool/host). Deployment report says client PR is blocked on worker rollout. | Width acceptance needs supported exact capacity, running worker controls. | Retain widths1/2/4 as individually unrun; no topology override. | 2 | Open: exact-cpu-request-31 / runtime owners |
| `fn arguments(args: Args) ... uses {}`; checker CAP-1 at arg.len/text/path (check-02.log). | Args reads charge Args authority. | Infer authority on this helper. No runtime/copy cost. | 1 | Intended / application |
| `Found(path) => ...path.cost` and `contains(path: Path)`; checker LANG-4: "before `.`, `path` names a module". | Module roots take precedence before a dot. | Rename local to route_value. No runtime/copy cost. | 1 | Intended / application |
| First full-app Rust test returned "language diagnostics" before execution; the initial source was not yet Hirsel-formatted. | Only completed execution can qualify a cell. | Retain attempt01 and pre-consumer source/tool seal; correct and format before new readers. Initial cells selected, none started execution. | 4 | Fixed preparation failure / application |
| Original `edges-cap`/`edges-over`/`closures-cap`/`closures-over`/`input-cap` main workflows, both evaluators: `ToolingLimit { kind: "Steps", limit: 1000000 }`, std@1/json.hir:135:9,138:38,28:38 (simulation-02.log, ten receipts). | Tooling stops are not completed semantic app results. | Keep inputs/budgets byte-exact. Native-only recovery uses explicitly independent cap oracles; both evaluator replay cells stay INCOMPLETE. Preserve 88 completed evaluator cells without rerun. No implementation workaround, copy cost or cap increase. | 3 | Open / evaluator std JSON budget; route-planner-33 owns retained original vectors |
| Native Toolchain wrappers capture stderr as application stderr. | Exact stderr parity must exclude cap tooling diagnostics. | Reviewed tools/capped bytes embedded; run_capped.py retains per-launch cap log and forwards only the inner application's streams. Adds two Python processes and temporary stream files, no app effects. | 2 | Intended / test harness |
| `Toolchain.wrapper=["python3", run_capped.py, ...]` under public launcher's env_clear; first native launch reports no observation, stderr "Failed to find executable : Is a directory" (native-01.log). | Helpers must resolve their own interpreter/tools under a minimal environment. | run_capped sets PATH=os.defpath for its tools. No app grant or provider change. Adds no process beyond the already documented wrapper. First native launch incomplete; earlier Heap differential and 88 evaluator cells retained. | 2 | Fixed / fixture helper |
| Native02 first live LLVM result matched stdout/status and tape stderr, but the test selected `Request::FileWrite` and saw empty payload. | AtomicFile `file.write_all` records `Request::WriteAll`; ordinary File writes use FileWrite. | Fix the test selector, require one Commit, and reuse the immutable original LLVM tape instead of repeating its successful live cell. No application or runtime change/cost. | 2 | Fixed / test harness |
| Missing final tape event refuses with `MissingInput(11)`, not `ReplayMismatch` (native03.log). | Missing, mismatched and extra events have distinct refusal categories. | Assert MissingInput. Retain and reuse the four successful first-tape replay cells and the binding/request controls. Checkpoint replay completions before remaining controls. No app/runtime cost. | 1 | Fixed / test expectation |
| Remote source sealing cannot assume Git or a .git directory. | A pool test sees only declared inputs. | Seal the declared sandbox input inventory when Git is unavailable; explicitly report that narrower scope. Host acceptance retains full tracked+owned source and evaluated generation stamps. No app cost; test hashing/copy overhead only. | 2 | Fixed / evidence helper |
| Extra-event replay returns exit3 with an empty Launched.record/stderr, while the public launch directory's `refusal` contains `refused the replayed tape: UnconsumedInput(12)` (native04.log). | Exit3 refusal and its exact valid stdout prefix must be checked, even when the observation file is empty. | Read the retained refusal file as an additional diagnostic witness; no runtime/provider patch. One small test-only file read. Preserve the missing-event four-engine control and original replay greens. | 2 | Local adaptation / public native launch diagnostics |
| Original nodes-over LLVM live: exit2/clean audit/stdout empty and tape PrintStderr correct, but Launched.stderr empty versus expected `routeplan: nodes\n` (native05.log). Runtime launch.rs constructs RealIo::fixture with stdout only; io_real.rs defaults stderr to sink. | Actual full-main live stdout/stderr/status/file bytes must match. | Keep the exact raw-stderr equality as an INCOMPLETE native-live obligation, never a pass; continue remaining independent native/replay cells and compare exact tape stderr. No runtime patch. Track each affected backend/case. | 3 | Open / native-file-faults-33 + host-replay-parity-31 (messages651164 and owner confirmation) |

## Qualification dispositions and recovery

The frozen CLI/schema and all43 original vectors predate implementation. Six
supplementary vectors were added before their first execution; none replaces an
original cap vector. Source checking and V export are tracked separately from
execution. The handwritten application has six reachable modules; no compiler,
std or runtime source was changed.

The initial evaluator matrix completed88 cells across49 vectors and two engines.
Ten exact original cells stopped at Steps1000000: reference and checked H for
`edges-cap`, `edges-over`, `closures-cap`, `closures-over`, `input-cap`.
Their original programs are the complete `examples/routeplan` entry with the
named bytes in `tests/support/route_planner_33/cases.json`, Args
`["data","network.json","routes.json","4"]`, a read-write data mount and a
preexisting `routes.json` containing `preserve`. Native-cap expectations in
`native-cap-oracles.json` are independent expected results, never evaluator
receipts. Each recorded backend's two evaluator replays remain incomplete on
these same five inputs. Bounds and original source remain unchanged.

The four helper cells (65536/65537 output bytes, both evaluators) pass alongside
explicit corrupt-predecessor controls. They do not discharge the original
full-main input/edge/closure obligations. Two existing public-Fixture regressions
(Pending sibling cancellation and AfterWrite release) also pass once through
actual Buck routing. The application itself does not need an AfterWrite gate;
its atomic report publication is not a write_bytes release.

Ordinary native fixtures use Ordered width1 by default; no physical-capacity
qualification is inferred from those semantics. All18 capacity cells, LLVM and
Cranelift x success/domain/Pending-cancel x requested widths1/2/4, remain
INCOMPLETE. Owner: exact-CPU deploy (@agentlang-24), confirmed in messages650047
and650050: kiln PR86 and build-infra PR94 await deployment; Hirsel client PR1 is
draft. No topology override, fake control reply, Free mode or CPU reservation is
used. Each deferred command is listed in the retained `ordered-reruns.md`; the
per-cell `ordered.py --binary ... --root ... --fixture-root ... --width 1|2|4
--kind success|domain|cancel` runs actual main, retains inotify readiness and
control replies and refuses any missing running/effective=requested/pending0
witness. Its host-signal cancellation is additional evidence; it cannot replace
the public-Fixture Pending witness. Capacity scripts are prepared, not executed
or claimed as validated against deployed exact infrastructure.

Tests deliberately use `-c hirsel.tests=host` so the chosen executor can retain
all tapes/source/tool/config closures outside ephemeral action scratch at
`/workspace/code/hirsel-briefs/route-planner-33/`; Rust compilation remains on the
ordinary shared pool. Every original attempt, including failures, has an EXIT
marker and retained log. Existing successful identities are reused during
recovery, using explicit BASELINE/RECOVER_ROOT/COMPLETED_* environment selectors.
No failed child or tooling stop becomes a semantic pass.

The pre-build seal includes tracked+owned source, evaluated generated BUCK inputs
and sync-stamp files/probes/globs/outputs, effective configuration, and helper
source. Each execution package seals actual CLI/test binaries, std image, native
runtime archives, LLVM/linker and link-kit trees before native consumers. The
additional Rust/Buck tool byte seal was late for initial Rust compilation and
is retained as late; it is not retroactively called a complete pre-build seal.
The source and tool byte union is retained before worktree removal. Rebased
landing trees get their own seal through the prepared generation wrapper.

The native suffix diagnostic gap (existing-empty observation masks refusal) is
owned by native-file-faults-33 (message650845). Our tests read the retained public
fixture refusal file as a local workaround and require the exact recorded valid
stdout prefix on late refusal. No competing runtime patch is included.

The final pre-landing matrix contains522 completed cells out of606 planned
full-main cells. Its84 incomplete cells are30 evaluator Steps1M identities
(ten live plus twenty tape replays),36 native live raw-stderr identities, and18
unrun exact-capacity identities. All98 native recordings and196 native replays
complete stdout/status/file/tape-stderr assertions;176 evaluator tape replays
complete, and20 stop at the unchanged budget. All16 refusal engine cells pass
(four controls x four engines). The scoped Pending read is demonstrably Blocked
before delivered cancellation and joined cleanup on both native backends.

The raw-stderr vector IDs are `nodes-over`, `edges-over`, `queries-over`,
`closures-over`, `version`, `revision`, `endpoint`, `query-endpoint`, `closure-id`,
`alternative-id`, `input-over`, `negative`, `bad-json`, `denial`, `pending-cancel`,
`jobs-0`, `jobs-5`, `alternative-not-primary`, each on LLVM and Cranelift.
Runtime owner accepted the fixture-only sink seam (message651187, relayed by
native-file-faults-33). This lane keeps its final aggregate red while any raw
stderr equality is incomplete; tape semantics never substitute for it.

Recovery after the owner fix must select only these36 raw live cells, use new
recording identities if the runtime revision changes, and preserve all original
success/replay/control evidence. Each original input remains in cases.json;
`ROUTE_PLANNER_33_CASE` selects comma-separated IDs and
`ROUTE_PLANNER_33_BASELINE` can bind the retained exact-source evaluator baseline.
`ROUTE_PLANNER_33_RECOVER_ROOT` reuses an existing immutable recording;
`ROUTE_PLANNER_33_COMPLETED_REPLAYS` takes `id:Llvm,id:Cranelift` pairs and skips
only their already-evidenced four replay cells. `COMPLETED_CONTROLS` and
`COMPLETED_DIFFERENTIAL` preserve separately completed controls/Heap checks.
These recovery selectors are operator assertions backed by retained receipts;
they never manufacture newly completed executions.
