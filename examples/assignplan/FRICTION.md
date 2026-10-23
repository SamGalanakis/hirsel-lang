# Assignment planner friction

Initial contract and 51 complete-main workflows were frozen before source under
`hirsel-briefs/assignment-planner-34/`. Evidence is not an application dependency.

* Intended adaptation, app owner: NetworkX matching.py:120–182 at
  9d866a5f447d9fe1c989b31f90c520dc2813ee26 uses dictionaries, a None vertex and
  nested mutable closures. Dense explicit unique inverse lists, sentinel distance
  33, and scoped paired mutable callbacks implement the same BFS/DFS layering.
  Cost: bounded linear edge scans per visited job; recursion depth at most 32.
  Jobs are the declared left partition, avoiding inference ambiguity. The generic
  Eligibility[T] record holds a borrowed row predicate and uses a stateless function and borrowed
  exclusion context as a separate argument. No borrowed row clone is needed. Active implementation minutes
  and exact diagnostic witnesses are recorded below when encountered.
* Intended adaptation, app owner: task.map_all's public result is copyable+inert;
  the planner freezes immutable inputs and explicitly deep-owns the aggregated
  certificate results before publication. Cost: one bounded final deep copy.
* Open qualification, coordinator / exact-cpu-request-31: real Ordered native
  capacity widths 1/2/4 need supported exact CPU admission and a running,
  effective=requested,pending0 control reply. No topology override is permitted.
* Open native fixture raw-stream/refusal/fault qualification,
  native-file-faults-33: preserve exact original failures until its repair lands.
  This lane makes no runtime patch and does not relax stderr or prefix oracles.

| Minimal original / exact response | Expected rule | Adaptation / cost | Minutes | Disposition / owner |
|---|---|---|---|---|
| `type Pair { resource: u64, job: u64 }`: LANG-14, "`resource` is reserved for a type declaration; choose another name" (simulation-g1.log). | Keywords cannot name fields or bindings. | Internal field/bindings use slot; tiny json_access/from/to adapters preserve the original frozen resource wire key. Two numeric field conversions per pair; no new codec. | 3 | Intended / app |
| `loop { ... }` is absent from Hirsel syntax. | Ordinary infinite iteration is `while true`. | Use while true; no runtime/copy cost. | 1 | Intended / app |
| Outer-shell ASSIGNMENT_PLANNER_34_EVIDENCE was not forwarded by Buck test action; first archive at /workspace/tmp. | Test executor environment is explicit via `-- --env KEY=value`. | Move the retained original archive into the evidence lane after its reader exits; use explicit executor env for future readers. No app cost. Original pre-setup seal is partial for actual Rust tools (captured at fixture entry), not retroactively complete. | 2 | Fixed / harness |
| `let result = one(item)?`: LANG-14, "`result` is reserved for the returned value in a postcondition; choose another name" (check-02/response.log). | Naming rule, not compiler bug. | Rename binding certificate; zero runtime/copy cost. | 1 | Intended / app |
| `Eligibility[model.Pair](...)`: LANG-2 "type arguments are never written in expressions"; unsuffixed branch `1`: "literal needs an expected type or numeric suffix". | Prefix inference and constructor syntax are intentional. | Written binding type with Eligibility(...); 1u64. Zero runtime cost. | 1 | Intended / app |
| Borrowed `for item in batch.manifests` then unique return: MODE-2 "this copy ended the owner's nested uniqueness". | Iterating a counted list copies its iterator owner; ordinary dropped aliases do not restore uniqueness. | Indexed borrowed validation calls preserve unique batch without a deep copy. | 1 | Intended / app |
| JSON derivers in report for model.Pair and explain.Certificate: LANG-4 unresolved model.pair_to_json/explain.certificate_to_json. | Deriver helpers resolve in the type's declaring module, with ordinary visibility. | Move/export those helpers in model/explain; no runtime/copy cost. | 1 | Intended / app |
| Eligibility record containing `fn(row) { !disabled(manifest, row.slot) }`: CLO-4 "manifest is local to the caller, but the closure escapes" (check-04/response.log). | A stored function owns its captures; a borrowed manifest cannot escape in that field. | The generic predicate takes borrowed excluded ordinals explicitly alongside the borrowed row. No capture, row clone, ownership transfer or extra allocation. | 2 | Intended / app |
| Original full-main allowed-cap and input-cap: both evaluators return `ToolingLimit { kind: "Steps", limit: 1000000 }` at std@1/ascii.hir:62:3 and std@1/json.hir:28:3 (simulation-f2Cfjw/receipts.json). | Tooling stops are incomplete, never language outcomes. | Preserve original source/bytes/budget. Independent analytic zero-matching cap expectations support native-only qualification; evaluator live and tape replay obligations remain incomplete. No implementation adaptation or cap increase. | 2 | Open / evaluator JSON step budget, assignment-planner-34 retains witnesses |
| Initial fixtures reused jobs-over for cardinality33 and CLI width5. Original evaluator receipts passed both (107181 and1090 consumed), but the same-ID oracle filename retained only the latter. | Every distinct vector needs a stable unique archival ID. | Rename only CLI width5 ID to cli-jobs-over, preserving every original input byte and Args; fixture-identities.json binds old/new IDs plus SHA256 and Args. Pending native-g3 was canceled by exact own client PID before any test started (EXIT141); no accepted native cell repeated. | 3 | Fixed / harness |
| Helper source had two statements on one line: LANG-1 "expected newline or semicolon after complete statement/item". | Statement separation is required. | Add newline, format helper. The failed formatter was a setup failure; helper had not executed. | 1 | Fixed / harness |
| Helper repeated `unique good`: OWN-2 "good was moved, but it is read here" (helper-precheck/response.log). | Unique formation may move an owned supplier; repeatable fixtures must request copies explicitly. | Test-only unique copy good creates bounded independent corrupt certificates. No application cost. | 1 | Intended / helper |
| G5 old-runtime replay helper unwrapped Observable.final_state: None at line923 after replay returned Ok and exact status/stdout/failure/blocked assertions passed. | Host replay returns semantic observations without a simulated final tree (observation.rs:400); missing semantic observations return Err separately. | Keep reference replay completion (source-line proof, accepted shepherd655035), skip it in recovery, compare exact immutable tape PrintStderr and require full consumption. Run only remaining checked-H replay. Real simulation final-tree assertions remain strict. | 3 | Fixed / helper |

| Original native g4 action reached unchanged600s action timeout after76 backend receipts plus endpoint LLVM live and two native replays. | Wrapper or timeout is not a pass; each completed identity is retained independently. | Preserve all raw tapes/402 initial identity records and four four-engine refusal successes. Recover only missing and stderr-affected cells in serial bounded selections; no action/process cap increase. | 3 | Fixed execution selection / app lane |
| G5 remote link action652d1566: RE TLS EOF, zero tests ran. | Infrastructure stop gives no semantic credit. | Retain original EXIT2; retry g5b only after scheduler recovery. Coordinator identified the authorized scheduler restart. | 1 | Infrastructure / rollout34 |

Old-runtime readers exited before rebasing onto main with593792b7/c6ebee19.
The remaining original endpoint checked-H replay and all four helper cells passed
in g5c; no original tape was changed. Native fixture raw stderr is repaired on
ABI53. New recordings have new identities and never recover old incompatible
bytes. Earlier successful live/replay/refusal identities are reused, while raw
stderr-affected and missing cells get bounded new runs. Four added public
provider-fault vectors use exact request sizes and error77, including a13-byte
partial atomic write. Simulation checks exact final files and no temporary leak;
native tapes check requests, fault outcomes and commit presence/absence.

| Seal helper matched ASSIGN_PLANNER_ while executor settings use ASSIGNMENT_PLANNER_34. | Effective selector configuration and external baseline bytes belong in the closure. | Fix the prefix and capture PYTHONPATH. Original selector commands are retained. Baseline values are backed by exact individual oracles; the external aggregate baseline copy was late for generation07 and is explicitly marked in baseline-seal-limitation.json. No accepted execution repeated to improve capture. | 2 | Fixed metadata capture; historical partial/late boundary retained / app lane |
| Copied grants used host examples/assignplan/fixtures. | Relative hosts resolve against the grants file (native-verification.md), not the shell CWD. | Use host fixtures and an explicit manifest path in README; standalone CLI record/replay smoke checks this route. No app runtime/copy cost. | 2 | Fixed app configuration |

Qualification uses explicit host test execution so archives remain reachable
outside test scratch. Native launches use embedded reviewed tools/capped bytes,
60CPU-s/2GiB per-process limits, and separate cap/application stream capture.
Free remains off. The original full53-vector native action hit its unchanged600s action budget.
The current57-vector unselected aggregate is UNRUN, never a passing aggregate. All missing and stderr-affected
inner identities were recovered in five serial selections of at most six cases.
The exact original timeout, individual receipts and all raw tapes remain retained.

| Standalone CLI smoke wrapper lacked its stage-local capped helper: python cannot open grants-smoke/capped; zero app entry. | Every host executor needs the reviewed helper bytes available before launch. | Embed tools/capped and wrapper in the smoke directory; retain exact staging-source/grants/input/tool closure before retry. No app cost. | 1 | Fixed smoke setup |

| Standalone smoke used the earlier dev CLI with the new std image: tools/capped stopped before app entry at60s, peak1195MiB, exit124, no recording. | Actual compiler and std-image identities must agree. | Resolve the CLI from the current successful fixture closure (checked CLI576edc18), not a guessed old output path. Same full app/input/process caps; retain mixed-tool original INCOMPLETE. | 2 | Fixed smoke tool binding |

The final union has57 actual-main workflows. Reference/checked live execution
completes110/114 cells; four original cap cells remain Steps1M. The114 required
native live cells pass across the retained unchanged-success and repaired-runtime
selections. The21 earlier ABI52 raw-stderr failures stay false in the ledger,
with explicit ABI53 replacement tape pointers. There are135 native recordings,
270 completed native replays and262 completed interpreter replays; eight original
cap interpreter replays remain Steps1M. All16 refusal engine controls, four Heap
differentials and four output-cap/corrupt-certificate helper cells pass. The two
frozen-oracle test runs cover their separately frozen53/57-vector unions.

All18 physical-capacity cells remain individually INCOMPLETE: LLVM/Cranelift,
requested widths1/2/4, success/domain/cancel. Coordinator654086 and later rollout
handoffs retain exact-CPU deployment ownership. Exact per-cell commands are in
hirsel-briefs/assignment-planner-34/ordered-reruns.md; the prepared ignored test
uses real main, control replies and delivered cancellation, with no topology
override or CPU reservation. This is capacity proof only; no speedup claim.

The grants-file route also passes a real CLI recording and its own offline replay
with the input directory unavailable, exact report/stdout/stderr/status, immutable
tape bytes and no host mutation. Evidence contains original setup failures and
all late/partial seals. No compiler, std or runtime patch belongs to this app.
Approximate active port work: layering/augmentation8minutes, owned batch adaptation
4minutes. Dense-array scans and bounded std task admission/chunk/final ownership
copies are the explicit costs of the legal language adaptation.

The deferred capacity helper preserves the actual child PID for delivered signals,
applies60CPU-s/2GiB OS limits, and reaps an unfinished child on harness exit.
This is prepared support code, not an executed physical-capacity claim.
