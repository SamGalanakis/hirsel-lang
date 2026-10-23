# suite-runner-29 findings

Base: 8453bcf1e04d58bba305cfc3c74e6343669b041a. Evidence, frozen drafts,
seals, exact argv/grants, receipts and immutable tapes live externally under
`/workspace/code/hirsel-briefs/suite-runner-29/`.

- Scope disposition (owner instruction): create a new examples/suite-runner;
  do not modify testrun. Its public commands, fixtures, test registration and
  packaging remain unchanged. The new example offers compatible stdout-only
  streaming; the suite path uses the existing both-pipe wait recipe.
- Ordinary ownership plumbing: decoded strings are copied into bounded case
  records; each admission constructs a fresh unique Command. Counted errors move
  out of nested indexed Results. Cross-task results contain inert status data;
  parent Scopes never cross closures. Cost: bounded argv copies at decode and
  spawn. No language relaxation or new runtime/std API. Active effort ~45 min.
- Inherited batchrun argv8192 remains independently INCOMPLETE at 1M steps;
  this application chooses 4096 before testing. No budget increase or argv8192
  repair. Boundary cells keep their actual tooling disposition.
- Checked-H host-tape replay has no public supported adapter. It is explicitly
  UNSUPPORTED / INCOMPLETE, not a replay pass. Full scripted app execution uses
  the public run_observed_checked adapter.

Verification dispositions will be filled from retained receipts; no all-engine
or replay completion is claimed before those obligations finish.

- Draft 01 (intended LANG-14, ~3 min): `let result = run.suite(...)`
  was rejected because `result` is reserved for postconditions. The same name
  appeared in helper bindings/fields; renamed to `value`. No allocation cost.
  Original source and full diagnostic: external draft-01 and workflows-01.log.

- Drafts 02–07 (intended syntax/authority/ownership, ~15 min): an assignment
  match arm needs a block; generic record constructors infer arguments from
  context; locals cannot shadow locals or top-level declarations; Args calls
  need their declared authority rather than `uses {}`. LANG-1/4 and CAP-1
  diagnostics are retained in check-02..07.log. `take decoded.value` reported
  OWN-8: move through a whole-owner pattern. `model.into_value` now consumes
  Outcome and returns the nested Result, transferring counted decode errors.
  This adds no copies. Cross-module record patterns use contextual type names;
  the model-owned consuming helper keeps this contract explicit.
- Derived JSON (intended std convention, ~2 min): nested nominal fields require
  `row_to_json` and `attempt_to_json` helpers; LANG-4 gave the exact missing
  signatures. Added the two real codecs. No new codec or std implementation.

- Semantic accepted-cap tooling stops (OPEN tooling, ~10 min): unchanged
  cases-32 and argv-4096 reach Steps 1000000 in std substring.hir:131:42 and
  :134:7; input-262144 reaches it in json.hir:30:8, in reference and checked H.
  Native acceptance and the one-over rejections are separately verified. No
  budget is raised or smaller substitute credited. Witness: external
  suite-runner-29-script-FGplnd/receipts.json and permanent caps.json. These six
  obligations remain INCOMPLETE; harness reports that disposition explicitly.
  Owner: suite-runner follow-up; std optimization is outside this lane.
- Reference cap replay refusals (OPEN/unclear, ~5 min): the native cap tapes
  separately return UnconsumedInput(7/8/70) in reference replay. Direct scripted
  step-limit results do not establish this replay refusal's cause. Exact
  originals: suite-runner-29-native-Wdpt8r/evidence/*-{cases-32,argv-4096,
  input-262144}.tape and replay-reference.stderr. Native replays and native live
  acceptance retain separate PASS credit. Owner routing: @agentlang-24.
- Short socket path (harness setup, ~5 min): the external evidence directory
  made AF_UNIX control paths exceed Linux's limit. Python reported
  `OSError('AF_UNIX path too long')`; harness termination produced empty JSON,
  not an app deadline result. Original host failures are retained; no fixture
  PID survived the subsequent audit. Use a private short temporary socket root;
  resume only the thirty failed native host/control identities. No app change.

- Cutoff fixture completion race (harness expectation, ~5 min): a child's
  `.done` precedes its parent's observation. Releasing the held successful child
  could let that completion be awaited first and admit the queued case before
  observing its failing sibling. Exact llvm-host-cutoff-w2 and cranelift
  host-cutoff-w4 reports/PIDs are in native-5IE7iT; all direct children were
  reaped. Expected contract is stop admissions after an observed failed
  attempt, not after an unobserved external exit. Change the held fixture to
  fail as well: either completion now trips the threshold with jobs fixed at
  two. Resume only those two failed identities. Other greens are retained.

## Verified dispositions

The application has 992 handwritten Hirsel lines: main 240, manifest 240,
select 77, run 211, report 147, model 77. All six modules appear in hirsel.json.
No compiler, runtime, ABI, std or original testrun source change is included.

- 22 small complete workflows (including two compatibility controls) PASS in
  reference, checked H, LLVM and Cranelift: 88 observations. The reference and
  checked-H observations compare complete output, exit, final trees/handles,
  failures and ordered effects, not just a pure helper.
- 28 cap/control vectors: 25 PASS in reference and checked H (50 observations),
  3 accepted-cap cells per engine remain INCOMPLETE at Steps 1000000. All 28
  PASS in each native backend (56 observations); no boundary was reduced.
- 42 host workflow cells PASS across reference width1 and both native backends
  at requested widths1/2/4. Success, nonzero, cancellation, attempt deadline,
  both pipes >capacity, and cutoff are covered. Handshakes observe child start,
  held/released siblings and cleanup. All observed direct PIDs are gone after
  return. The 30 native held-child cells retain production control replies
  with effective widths1/2/4 and fixed logical jobs2. Pipe-only cells retain
  requested width, without an effective-width claim for their short lifetime.
- 144 complete immutable tapes (including original legal cutoff-race outcomes)
  were each replayed by reference/LLVM/Cranelift: 426 replay cells PASS, six
  reference cap-tape replay refusals remain INCOMPLETE. Replay removes the
  original input/report directories and private child program, asserts no
  real effects and immutable tape bytes, and compares stdout, application
  stderr and exit. Supported replay completion enforces event consumption.
- Nine changed-input/config/tape controls refuse across the three replay
  adapters. Original tape bytes remain unchanged. Checked-H host-tape replay
  and checked-H timed/host-backpressure workflows remain UNSUPPORTED /
  INCOMPLETE: public ChildScript has no timed step and no public host-tape
  checked-H adapter exists. No new adapter was built.
- Cross-width equality of deterministic native script tapes is INCOMPLETE:
  native monotonic clocks are independently sampled live inputs. Each tape
  agrees with its own replay; clocks/task IDs/errors are not normalized away,
  and no sorted event comparison is presented as cross-width evidence.
- Three new Rust test identities (0 before / 3 after); current CLI harness
  lists 387 tests (384 pre-change inferred by subtracting the three additions).
  The original testrun focused test PASS (one selected, one executed); original
  examples harness still has 98 tests. Runtime partitions retain all earlier
  failures. Known tooling stops are explicit INCOMPLETE receipts, never app
  observations or replay passes.

External authoritative receipts: workflows-03.log; caps-01.log and
suite-runner-29-script-FGplnd/receipts.json; native-Wdpt8r/evidence (initial),
native-5IE7iT/evidence (short socket recovery), native-nkxJe1/evidence (only two
cutoff failures), native-SAFdBN/evidence plus refusal-receipts.json
(compatibility and binding controls); script-compat-04.log;
complete-stderr-final.json; ordered-widths.json; race-replay-receipts.json;
testrun-preserved.log. Directory names abbreviate the `suite-runner-29-` prefix.
Replay diagnostic masking owner: reference-replay-29, coordinated with
packtree-29; direct step-cost owner: interp-step-budget-29. Original refusals
stay incomplete until their exact obligations are independently satisfied.

Sealing: initial setup seal omitted the actual .buck2/sync-stamp.json. No
semantic observations passed that boundary. Seal03 corrected the full
tracked/owned source union, actual generated.BUILD_INPUTS and stamp
reads/probes/glob membership/output union, effective configuration, resolved
compiler/image/runtime/LLVM/link-kit/helper/std/oracle bytes, modes and symlinks.
It and each affected later seal were verified after readers exited. A later
nested-directory-symlink audit found none in these tool trees. Setup checks
before seal03 are honestly setup-only, not retroactively sealed executions.
All app Hirsel bytes have remained unchanged since the first accepted runtime;
only harness/support and docs changed at joined reader boundaries.

Reference deviations and license provenance are in README.md and LICENSES.md.
No optimization or instruction-count claim is made.
