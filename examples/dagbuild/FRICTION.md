# dagbuild findings

Starting source: 8453bcf1e04d58bba305cfc3c74e6343669b041a.
External immutable attempts, input seals, workflow matrix and receipts:
`/workspace/code/hirsel-briefs/dagbuild-29/`.

Application plumbing: explicit nominal JSON hooks; owned command snapshots;
Shared immutable declaration table; generic String/cache and u64/task-slot
lookups; scope cancellation and joins; atomic file publication. No language,
runtime or std repair is included. Costs: bounded argv copies per admitted job,
bounded manifest DOM plus typed declaration copy, and conservative cache loss if
publication fails. These are intended contracts, not reported language defects.

Qualification remains **INCOMPLETE**. Full-app reference/checked-H:100 cells,
66 passed,34 incomplete (32 Step1M stops, one original reference allocation
failure, one unrun checked-H twin). Native scripted/boundary:108 cells,107
passed, one original LLVM diamond width4 timeout. Real-child Ordered:18 cells,
17 passed with control-proved effective widths1/2/4 and reaping, one original
Cranelift cancellation width4 timeout (topology unknown). Both native backends
passed all12 real incremental artifact/state oracle steps.

Across136 complete recordings,408 replay attempts: reference66 passed/70
refused; each native adapter128 passed/8 refused. The two live timeout tapes
have no complete trailer (six replay cells unrun). Refusals are retained as
INCOMPLETE by exact fixture/producer/adapter/exit/ReplayMismatch signature in
known-incomplete.json; unexpected failures remain red. A green disposition
harness never means every engine/replay cell passed.

Owners: `e1-live-hang-29` for live timeouts; `reference-replay-29` for reference
third-spawn mismatches; `interp-step-budget-29` for accumulated file.read
snapshots and the original1MiB reference memory failure. Native replay of
child-created artifacts refuses at `file.size` (first30,input33); routed to
@agentlang-24 for ownership, distinct from reference tooling stops. Exact raw
source/argv/grants/tape/tool hashes remain external. No failed cell was rerun
unchanged, no cap was raised, no smaller smoke replaced an incomplete vector.

Checked-H host-tape replay is unsupported/INCOMPLETE: no public adapter exists.
Native clocks are independently sampled; each tape was compared with its own
recorded output/exit and complete event-consumption adapter, with original
files/helpers unavailable. No clocks/taskIDs/errors were normalized away. A
fixed-clock native cross-width comparison is unrun/INCOMPLETE. The closure
oracle validates process-spawn ready membership, ancestor success and bounds;
ordinary tapes lack parent task-admission/join anchors, so joined-boundary tie
qualification remains INCOMPLETE (not inferred from physical child starts).

## Source and harness findings

Each witness below is external, under the evidence directory above. No language
rule, compiler, std API or runtime ABI was changed.

| Minimal source / behavior | Actual contract or diagnostic | Adaptation and cost | Effort; disposition; witness |
| --- | --- | --- | --- |
| `Record { result: String }` | LANG-14 reserved postcondition name | Explicit pre-runtime schema revision: JSON field `digest_result`; domains remain `dagbuild-1`/`dagbuild-result-1` | 2 min; intended; attempt-02/check-03.log, CONTRACT.md |
| `fn key(...); fn check(key: String)` | LANG-4 local shadows module value | Rename function `compute_key` | 1 min; intended; attempt-03/check-04.log |
| `lookup(...)->Option[V] { Some(copy row.value) }` | MODE-1 returned value must be global | Declare result `in rows`; cache records remain borrowed, avoids deep copy | 2 min; intended; attempt-03/check-04.log |
| `Ok(take dag)` after declaration views | MODE-2 deep uniqueness not proved | `Ok(unique dag)`, bounded declaration copy | 2 min; intended; attempt-03/check-04.log |
| Task result with String reason; shared.with Command | MODE-4/CLO-3/MODE-2 result/capture not sendable/unique | Plain numeric reason formatted by parent; explicit unique owned command snapshot, <=4096 argv bytes/job | 3 min; intended; attempt-04/check-05.log |
| Rust `ObservedSimulation.files` | E0609; actual state is `trees[0].1` | Use public actual adapter shape | 2 min; fixed harness; test-01.log |
| CLI `check manifest --json` | invalid option | `check --json manifest` | 1 min; intended; check-02.log |
| Buck `--local-only` with remote-only compiler platform | incompatible executor; NO TESTS RAN | Existing `-c hirsel.tests=host`, remote builds + host tests | 2 min; tooling; native-07.log |
| Reference empty replay exits0, exact stdout, compiler EAGAIN warning | Host compiler worker creation failure, outside tape | Compiler HIRSEL_JOBS1 inside fixed4CPU wrapper; preserve raw diagnostic and recovered semantic disposition | 3 min; tooling; native-08.log and retained host-NWp8Qz |
| Clean dependency releases earlier-declared ready vertex | Initial for-loop could admit a later vertex first | Scan declaration order again after every clean release | 5 min; fixed application logic; attempt-09/execute.hir, clean-ready-tie vector |
| Larger full-app reference/checked workflows | ToolingLimit Steps1000000 in std SHA256 | Preserve exact vectors and incomplete cells; no raised cap/optimizer/smaller substitute | 5 min; OPEN tooling; test-06.log and reference-checked-06-receipts.json |
| Native chain reference replay | ReplayMismatch(31), third process_spawn | Preserve exact tape/source/binary/grants and continue independent native adapters | 5 min; OPEN reference-replay-29; host-LRY8OW, native-09.log; routed @agentlang-24 |
| LLVM diamond Ordered width4 | Capped60 timeout; no stdout, empty safely invalidated state | No repeat/cap change; preserve incomplete live/tape cell | 5 min; OPEN e1-live-hang-29; host-6KC7kP/evidence/llvm-diamond-first-w4, native-10.log; routed @agentlang-24 |

Initial tracked/source/generated-input stamps were sealed before consumers.
Exact runtime archives/std image/native helper files were additionally hashed
in `native-tools-late-seal.json` after the first native builds. This is explicitly
a late seal, not retroactive proof of their pre-build boundary. Later consumers
use the frozen artifacts and retained source/tool hashes.


Additional retained harness findings: audit12 initially confused child.wait
completion with parent admission, then audit13 matched one-input-change by
substring to input-change. Their original assertions/tapes remain external;
exact fixture parsing and a process-ready membership oracle pass the same
immutable tapes (audit14). Joined-boundary readiness remains unproved. About5
minutes; fixed harness, no application change.

Real14's longer control socket path exceeded sockaddr_un. Short owned /tmp
control paths repair only the missing cells; the three existing LLVM success
widths were retained. Real15 was canceled before any app launched to avoid
repeating them. About3 minutes; fixed harness, real14/15/16 logs.

Semantic caps13 accidentally inherited the7808M Buck harness reservation and
aborted the accepted1MiB reference fixture at6916M resident. This is the original
memory red, not a2GiB pass. Later missing-only cells explicitly used2GiB; the
large fixture/twin remain INCOMPLETE without rerun. About3 minutes; setup error
plus owned interpreter defect, semantic-caps13/14 logs. No increased cap.

The native Rust wrapper now has separate LLVM/Cranelift tests so each scheduled
partition stays under the existing600s test-action bound. This changes no app
or child commands. Previously completed backend receipts are preserved; only
policy/compiler checks consume the final wrapper edits.

Final exact-disposition tightening: `disposition.py` accepts only enumerated
scope/producer/fixture/width/adapter/exit/ReplayMismatch positions from immutable
original receipts. Offline controls checked83 retained refusals and reject an
altered diagnostic; no native workflows were repeated for this policy change.
The final harness compiles, and missing tape-audit configuration fails setup.
Refusal controls passed for changed argument snapshots/configuration, changed
source configuration, corrupt tape and truncated tape (seven controls total).
The original wrapper name was split by backend; test inventory384 ->389 (five
new tests, two explicitly ignored external-tape helpers), preserving every
unrelated registration. All raw counts and per-generation observations remain
in the external FINAL-SEMANTIC.json and FINAL-RECEIPTS.json.
