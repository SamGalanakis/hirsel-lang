# depsolve authoring and qualification friction

Starting SHA: `8453bcf1e04d58bba305cfc3c74e6343669b041a`.
Application-only lane. No compiler/runtime/std API, construct, ownership rule or
launch package-selection change. Approximate times below are active authoring
cost, not measurements. Immutable drafts, exact diagnostics, expected workflows,
source/tool/config seals, tapes and raw receipts live externally in
`/workspace/code/hirsel-briefs/depsolve-29/`.

| Attempt/minimal form | Actual diagnostic or behavior | Contract and retained adaptation | Effort/disposition/owner/witness |
| --- | --- | --- | --- |
| `Package { requires: List[Requirement] }` | LANG-14: requires is reserved for a contract clause | Schema/fields use `dependencies`; no workflow had executed before this schema amendment | ~2 min; intended; depsolve-29; draft01/02, test-02.log |
| `explain.Limit("jobs")` | LANG-4 unresolved name; enum constructors are contextual variants, not module functions | `source(Limit("jobs"),...)` has Fault context | ~2 min; intended caller correction; draft03/check-04.log |
| local `text` payload with top-level helper `text` | LANG-4 local cannot shadow top-level value | Name helper `source_label` | ~1 min; intended; draft03/check-04.log |
| generic borrowed `unique value` returned globally from Trace producer | MODE-1 local result cannot be global; unique preserves source placement | start/extend take unique global payload/trace; callers clone borrowed descriptors explicitly | ~3 min; intended; draft03/check-04.log |
| unique root `origin` in root-edge loop, passed as `unique origin` | OWN-2: origin moved/not initialized on every path | Construct a small constant root trace per admitted root edge; unique conversion moves an already-unique value | ~1 min; intended; draft05/check-05.log |
| task `Result[unique Loaded, Problem[String]]` | MODE-4 error Fault.Version(String) not proved sendable | Both Result slots preserve unique payload contracts; task owns filename/size descriptor | ~2 min; intended; draft05/check-05.log |
| `for item in owned.dependencies`, followed by sort | Compiler bug: the borrowed field loop was copied instead of lent (SEM-4 / MODE-2) | Direct borrowed validation loop, then in-place sort; no extra list copy | ~2 min; bug, fixed by e5e974f27a4c; draft03/check-04.log |
| report code/detail helpers return plain String | MODE-2 report's unique fields not proved unique | Private helper case fixed by private summaries (b552ff23d839 / f71ba1242e00); current exported code/detail retain their declared rows | ~1 min; draft03/check-04.log |
| `let mut values: unique List[String] = unique []` inside generic recursive flatten | MODE-2 at empty initializer: `unique []` not proved unique | Ordinary builder and `unique values` at return. Copies bounded flattened path strings/list once more per recursive level; no relaxation. Original form remains rejected on starting toolchain | ~5 min; open, owned by unique-empty-builder-29 per @agentlang-24; unique-list-witness.hir/.log and draft05/check-05.log |
| per-file typed catalogs then global graph bounds | Review found typed record allocations could precede global node/edge refusal | Preload raw bounded bytes/digests concurrently, then parent admits DOM identities/names/edges with run-wide Admission before typed decode | ~5 min; application correction; depsolve-29; draft07 and amendment-11.md |
| Admission names set mutated by helper without promise | MODE-2 empty set no longer proved unique at Admission return | Fixed by private summaries (b552ff23d839 / f71ba1242e00); private helper body proves mutable handback | ~1 min; check-10.log |
| removed decode match left partial arms | LANG-1 expected statement separator/top-level definition | Remove editing remnant; formatter/source check then continued | ~1 min; caller editing error; check-09.log |
| `state.files` in Rust harness | E0609 ObservedSimulation has stderr/trees/handles/root_trace | Assert exact tree file bytes; final logical handles retained in receipts | ~1 min; harness API mistake; test-01.log; no tests ran |
| detached prepare shell | PID gone, empty log, no .buck2/EXIT | Retained exec session with unconditional EXIT; normal prepare succeeded | ~1 min; setup only; prepare.log, prepare-02.log |
| `check PATH --json` | CLI usage exit2, invalid option position | `check --json PATH` | ~1 min; intended CLI syntax; check-03.log |
| Buck `--env` before executor separator | Buck usage exit3; no tests ran | `buck2 test ... -- --env NAME=value ...` as current help documents | ~1 min; setup only; test-08.log |
| native grants with args but no trailing app argv | llvm-empty-w1 returned exact usage report/status2 | Pass app argv immediately after grants operand; reuse both source-matched native builds | ~2 min; harness launch mistake; test-11.log, native generation syCNYh |
| short run's metrics file empty | periodic writer has not reached one-second sample | Inotify socket-creation subscription, then supported control metrics request; no sleeps or app-only probe mode | ~3 min; intended diagnostic timing; child.py, native admission.json |
| requested4/effective3/pending4 admission reply | Native output/status and normalized full tape already agreed; topology assertion failed | Retain physical cell INCOMPLETE. App semantics, replay, requested/effective admission and physical participation are separate claims | ~2 min; host/runtime disposition, @agentlang-24; topology-disposition.md, test-12.log, llvm-cross-w4/admission.json |
| full scripted workflows at default 1M steps | ToolingLimit(Steps,1000000) in std SHA-256/JSON/hex, no observation | Original exact inputs unchanged; no raised budgets. Full audits explicitly ignored/INCOMPLETE in permanent harness; small complete workflows still run via actual package/entry and public adapters | ~3 min; tooling; depsolve-29; test-07.log, test-11.log, full per-engine receipts |

The pure model never replaces application execution. The graph borrows catalog
Map values and owns BFS work traces. Trace/Problem have meaningful String and
Requirement instantiations; regular recursive parent chains never change T.
Sorted adjacency and filenames plus source-order awaits choose deterministic
shortest paths and errors. Cancellation joins remaining task-set members; ordinary
file-resource drops close every admitted handle. Read failures and cancellation
are typed application faults. Bounds refuse ordinary data, not language failures.

Source sealing: early pre-prepare/test01/test02 and build03 seals include content
hashes and recorded generation stamp, but did not evaluate every current
probe/glob membership; those limitations remain labeled in the external report.
Later pre-consumer seals evaluate stamp probes/globs/outputs/read unions and hash
actual nested compiler/std/image/runtime/tool roots, fixtures, argv and config.
Completed-source complements are verified. Late/more-complete seals never create
retroactive test credit. Native generation reuse checks all six source files
byte-for-byte and preserves original executable build outputs. Resume consumes
immutable receipts to avoid repeating completed green engine cells.

## Engine/replay dispositions

The original 36-case full public-adapter matrix attempted 72 cells per engine:
reference 52 semantic passes/20 tooling stops; checked H likewise. After global
admission changed, the affected small matrix attempted 62 cells per engine:
60 semantic passes/2 tooling stops (split, seeds0/7). Those exact split inputs
remain in the explicit full audits; they are not shrunk into a passing replacement.
Heavy full-workflow obligations remain INCOMPLETE on the final revision; earlier
stops and exact sources remain retained. See external matrix.json for per-ID state.

A native SimulationConfig filesystem adapter is unavailable. Native host-file
fixtures supplement simulation; they do not become scripted native fault-provider
passes. Checked-H host-tape replay is unsupported and INCOMPLETE. No incidental
runtime adapter is added. Native/replay counts and outstanding physical cells are
filled from final receipts below, never inferred from source/export success.

Final application source: **916 lines**, modules main 186, catalog 177,
model 294, resolve 95, explain 100, lock 64 (all additions, no removals).
The final small public-adapter selection has **62 passing cells per engine**:
60 retained from revision11 plus the two new fanout-small seeds from test16.
The latter is an additional independent tiny workflow, never credit for the
unchanged split input that exceeded the budget. Scripted canceled-read paths
pass on both adapters; they do not establish native provider cancellation.

Native host-file validation has **98 successful record cells** (49 per backend),
**294 successful replay cells** (every tape through reference, LLVM, Cranelift),
and **146 refusal controls** (98 truncated tapes plus 48 changed binding/input
controls). Replays run after the mounted input directory is unavailable, assert
full stdout/stderr/status, consume complete tapes, preserve tape bytes and leave
host inputs absent. An external independent byte audit confirms all 392 native
record/replay stdout documents exactly match the oracle's schema member order,
contents and final newline. Full normalized semantic tapes agree across requested
widths without sorting or dropping task IDs/errors/clocks; only admitted-at and
placement provenance are normalized. No app samples a live clock.

The original combined native test reached the harness's fixed ten-minute timeout
after many completed LLVM cells. Only missing identities were resumed in bounded
backend/workflow partitions, using immutable status/argv/tape receipts; an
unfinished pre-consumer seal was retained as setup-incomplete. Caps stayed 60 CPU-s,
2 GiB and four wrapper CPUs. Both native builds were reused byte-for-byte for
this source generation rather than rebuilt per partition.

**Physical qualification remains INCOMPLETE.** The retained generation predates
`worker_phase`; its first replies can occur while workers are being created. The
runtime owner reproduced requested4/effective3/pending4, then settled width4.
Updated support retains all bounded control samples and only qualifies a reply
with phase running, exact requested/effective width, pending0. Older-runtime
receipts cannot meet that stronger contract; no completed semantic campaign was
repeated for this diagnostic change. Ownership: e1-effective-width-29, coordinated
through @agentlang-24. These receipts do not claim physical parallel work.

**Other INCOMPLETE obligations:** full checked/reference scripted heavy cells at
unchanged 1M steps; native scripted filesystem/fault-provider adapter unavailable;
checked-H host-tape replay unsupported. Exact witnesses, argv/config/tool hashes
and original dispositions stay external. No engine run is inferred from checking
or export. The 8 new Rust registrations comprise 6 runnable focused identities
and 2 explicit full-audit ignores (existing harness inventory 384 -> 392). All 6
runnable identities have passing focused receipts; earlier failures and aggregate
timeout remain preserved rather than rewritten as greens.

Sealing limitation: the original test11 native builds have an outer tracked,
owned, generated-input/compiler/std-image/archive seal, but the launcher's exact
materialized link-kit/environment byte union was first fully captured afterwards.
That later seal is **late for those original builds** and gives no retroactive
pre-build credit. Subsequent native consumers have concrete environment/tool,
source closure, fixture/config/argv seals. Immutable tool digests may be reused
only with identical device/inode/size/mtime/ctime; directory membership is always
re-evaluated. This provenance obligation remains explicitly incomplete; builds
and unchanged greens were not repeated merely to improve sealing.

The shipped README fixture/grants also pass two standalone resolve/verify
recordings from another cwd plus six reference/LLVM/Cranelift replays with the
private mounted fixture directory unavailable (8 additional cells). The grant
file is copied unchanged, preserving its relative `host:fixtures` behavior.
These smoke receipts are separate from the 98/294 matrix counts above.

Resolution of the empty-builder workaround: `unique-empty-builder-29` landed
`4a4b9377674f6ed5be734030abc81f3c362466f8`. The original form was legal;
ownership and V lost independently proved consuming-loop item uniqueness.
The owner validated the exact witness and shrink/forgery controls across four
engines. The table's open disposition describes the original qualification pin;
the compiler defect is now fixed. This app retains its tested bounded-copy
workaround, with no new engine campaign or ownership-rule change.

Final evidence-helper review found that the test launcher supplies a relative CLI
path. Outer pre-test seals retain tool hashes, but individual consumer seals
previously scanned only absolute argv paths. The helper now resolves file argv
against the actual cwd and always hashes its own executed source. Original
individual relative-CLI seals remain incomplete; this repair is not retroactive
execution credit and causes no repeated semantic campaign.
