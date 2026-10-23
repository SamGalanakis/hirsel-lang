# rpcgate authoring friction

Base d49af62d; drafts and exact JSON diagnostics retained externally in
hirsel-briefs/rpc-gateway-30. No compiler/runtime/std repair so far.

- First check (about 5 minutes): LANG-4 module heads `table` and `route` take
  precedence over local dotted field access. Rename locals to manifest/endpoint.
  LANG-4 also forbids shadowing `fields` and `sender` in nested scopes. Intended
  naming rules; no allocation cost, application fixed. Owner rpc-gateway-30.
- OWN-8 first check: `take reply.body` cannot partially move its owner. Destructure
  the whole `Envelope` before rebuilding the downstream ID. Intended ownership;
  no extra copy. Witness draft-01/attempt.hir:107, check-01 log. Fixed locally.
- CONC-5 first check: capturing `token.upstream` or `state.jobs` directly in an
  inert mutex callback also captures their authority-bearing owner. Snapshot
  the bool/u64 outside the callback. Intended capability rule; no allocation.
- CONC-6 first check: an unqualified Bytes in Reply is not a sendable channel
  payload. Qualify the field `unique Bytes` and construct owned frame copies.
  At most2052 bytes per queued reply, <=16 credited replies; no brand erasure.
  Intended crossing rule. Original draft and check-01 retained.
- First check used the wrong SleepError shape. Actual API is Wait(Canceled(...))
  and Wait(Clock(...)), not bare Canceled or Clock. Fixed application match and
  mapper; ordinary API plumbing, under one minute.
- Build setup initially guessed nonexistent //:hirsel. Buck refused target
  selection, no source execution. Inspected generated BUCK and used
  //:hirsel-cli-bin-hirsel. Setup failure, not a language or engine failure.
- Check02: a qualified record pattern (`let wire.Envelope(...)`) is LANG-5;
  constructor lookup uses the expected scrutinee type. Use `Envelope(...)`.
  Intended pattern syntax, no copy, under one minute; draft-02 retained.
- Check03 CLO-1: a scope closure captures ordinary outer bindings read-only.
  Move reader/sender into the once scope body and declare mutable storage there;
  return the client count instead of mutating an outer capture. No extra copy.
  The record consume helper now lives in wire, where Envelope is directly named.
  Checked record patterns did not resolve a cross-module generic constructor in
  the attempted short spelling; no compiler defect is claimed. About3 minutes.
- First focused test selected/started one named body, then failed at app checking:
  LANG-2 on `Deadline(clock:c,at:at)` because the new SessionError arm `Deadline`
  shadowed core's record constructor. Rename the arm DeadlineRange. Original
  test-01 log/draft-05 retained; no engine cell ran. About2 minutes, intended.
- Application admission refinement after the first boundary cells: jobs initially
  refused whenever all upstream slots were occupied, so the16-request credit
  limit was not usefully reachable behind four blocked upstreams. Admitted
  requests now wait on the existing cancellable timer (10ms) while keeping a
  request credit. This bounds queued tasks at16, active sockets at jobs1..4,
  and shares the existing absolute deadline. Cost: one timer and inert mutex
  attempt per queued request per10ms. This is application policy, not a missing
  language/std primitive. The independent16/17 vectors first execute this path;
  prior successful cells had no upstream-capacity wait, so are not repeated.
- Admission01 setup: the fixture-edit Python draft had a mismatched parenthesis,
  so no app edit ran; the subsequent wrapper selected zero tests. Its exit0 is
  retained as SETUP FAILURE, never acceptance. Corrected the edit and verified
  named bodies in the next selection. Under one minute, application tooling.
- Admission02 preserved an over-specified peer oracle: queued requests' deadline
  replies can finish before a deeper blocked upstream's reply, and independent
  host clock samples need not tie. Expecting r0..r15 wire order was wrong for this
  concurrent protocol. Original LANG-9/LLVM abort and all4 reference/checked-H
  reds remain. The corrected peer supplies the same16/17 frames and stalls;
  the finite session deadline joins it, while sorted summaries and exact connect
  counts prove16 credits and the seventeenth refusal. This does not reorder any
  recorded events; downstream IDs are separately checked in live peers and
  out-of-order fixtures. About3 minutes; application oracle fixed.
- Admission03: reference/checked-H16/17 each passed, LLVM16 passed, LLVM17
  legitimately started later-deadline queued requests after earlier deadlines
  released slots. Its additional attempts were within jobs4, not a retry/reset
  of their deadlines. Original fixed4-attempt live-clock oracle was wrong;
  admission04 freezes4..16 unique first-attempt rows, deadline/transport only,
  plus exactly one seventeenth busy refusal. Each tape is compared only to its
  own recorded output. No semantic trace is sorted or clock normalized. LLVM16
  recording/native replay greens are reused; only its missing host replays run.
- Pending01 wrapper check: ExitCode is opaque plain/inert but does not export
  `data`, so equality is GEN-1. The wrapper observes its Expired arm and actual
  file.read.pending/file.close effects instead; it does not introspect or
  forge the application's status. Original source/log retained, intended rule,
  about1 minute, application harness fixed. The wrapper calls the real six-module
  app and supplies only an external parent deadline, not replacement gateway logic.

## Replay/provider disposition and evidence

- First28 full-entry reference/checked-H executions passed fixed stdout, exits,
  exact connect counts and full ordered-effect equality. The harness then
  removed files/net from SimulationConfig for replay and compared against a
  different simulation world: all28 comparisons were red. Original full Debug
  artifacts lived only in an unexported remote sandbox and were lost; the exact
  original field comparison is evidence-incomplete. Their receipt log and source
  seal remain. No live28 cells were rerun to regenerate those artifacts.
- A later replay-only experiment used retained reset-success semantic events:
  same-world checked replay passed; removing files/net produced no observation
  and `invalid TIR: capability replay mismatch at event 8` (file.open). Simulation
  event verification requires its initial world. This is an intended adapter
  distinction, not a new language failure. The full/absent result files are
  external. Production replay uses public `replay_observations` and
  `replay_observations_checked`, which replay initial file snapshots with the
  original files and providers absent. They execute the actual application.
- The first native14-case partition passed both backends and all four tape
  replay adapters. Its original raw tapes were not exported from the remote
  action; the aggregate receipt remains, but raw-tape retention is INCOMPLETE.
  Subsequent partitions run on the host and keep exact immutable tapes/source,
  grants, argv and output. This was an evidence setup error, about5 minutes.
- **OPEN runtime owner replay-scripted-net-30:** full native17-admission runs
  succeed, but the LLVM tape fails all four replay adapters with
  `ReplayMismatch(562)` and the Cranelift tape with `ReplayMismatch(565)`.
  The16-admission tapes pass all four. Minimal retained application witness:
  17 pipelined Arith.Add requests, jobs4,100ms request deadline,1s session,
  first four connections Unanswered, downstream Stall. Two exact tapes, original
  six-module source, grants/argv/tool hashes and failing replay outputs are in
  `rpc-gateway-30/retained/rpcgate-native-BDoKD8/llvm-in-flight-17` and
  `rpcgate-native-p3T1ir/cranelift-in-flight-17`; replay-witness.sh reproduces in
  under one second. Owner isolated a ScriptNet cursor not advanced by a replayed
  blocked Unanswered connect. Eight required replay cells remain INCOMPLETE
  until these exact immutable originals are validated. No parallel runtime fix.
  Active isolation/routing effort about10 minutes.
- **OPEN native-harness-gaps-30:** deterministic native full-Io/Args/file/Net
  fixtures and cross-width semantic-tape equality await that owner's public
  Fixture args/files/mounts recipe. Production live-clock runs are not relabeled
  deterministic or compared after dropping clocks/task IDs. The exact app and
  provider fixtures are the witness; missing deterministic cells remain
  INCOMPLETE. The app lane adds no runtime launcher/provider adapter.

33 scripted workflows execute the full entry/config/all modules on reference,
checked H, LLVM and Cranelift. Reference requests64/65 each consume2,217,098 steps
under their predeclared12M fixture bound; the global1M policy is unchanged. Other
fixtures declare4M. The native16/17 live-clock admissible row contract is above;
reference/checked H retain fixed exact4-connect outputs for those cells.

Real Unix providers additionally qualify success, application error, pending-read
cancellation and one externally visible non-idempotent action at normal Ordered
widths1/2/4, fixed jobs4. Every one retains a listening handshake then
running/effectiveN/pending0 receipt before release. These are capacity receipts,
not proof of simultaneous hardware execution. The backoff peer observes the
third armed timer through the public immutable-prefix tape parser, disconnects,
and asserts one connect/no second externally visible attempt and joined cleanup.
Those live tapes replay on all four engines after sockets/config disappear.

A parent-deadline fixture invokes the real app entry during an actual
FileReadScript::Pending configuration read; reference and checked H observe
file.read.pending then file.close, with no injected Canceled result and no Net
projection. Native pending-file coverage awaits the same public fixture owner.
Changed configuration bytes, Args, source revision and extra tape input each
refuse on all four replay adapters (16 refusal controls). No failed or unrun
cell is counted as a pass.

The pre-consumer source/generation/tool seals are external. Advanced01's before
and after boundary hashes match exactly. Earlier setup prepare/format probes
that preceded complete consumer sealing are labelled setup/late in the external
inventory; native executable hashes added after build are not claimed as
pre-build bytes. A portable3229-file original tool/std/runtime/image/link-kit
bundle was handed to the replay owner before rebasing, so later source movement
cannot silently replace original tooling. No compiler/runtime/std change was
made in this application lane.
