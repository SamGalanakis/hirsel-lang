# Pubsub relay friction

Original drafts, diagnostics, oracle vectors, seals, matrix and per-engine receipts
are retained under `/workspace/code/hirsel-briefs/pubsub-relay-30/`. Minutes below
are approximate active effort, not benchmark timing. No compiler, std, runtime,
language construct or ownership rule is changed by this application.

| Attempt / minimal form | Actual diagnostic or behavior | Expected contract; resolution / cost | Disposition / owner / witness | Minutes |
| --- | --- | --- | --- | ---: |
| Generic `wrap[T](take Credit, take T) -> Envelope[T]` followed by sending the result. | CONC-6 / MODE-2: result not proved unique; part extraction through a nominal variant weakens the nested proof. | A signature must promise the ownership its consumer needs. Final envelope carries a frozen payload root and one uniquely owned credit; producer freezes bounded command/message data, consumer makes one bounded unique copy. Real `Envelope[Command]` and `Envelope[Message]` instantiations remain. | Intended ownership contract; app owner; drafts/first, check1, check2, /tmp/pubsub-relay-30-check1..7.log. | 8 |
| `unique topic` used twice while subscribing; Mutex handle passed to a taken constructor and then reused. | OWN-2: topic/state moved, then read/captured. | Explicit `wire.owned(topic)` bounded deep copy and `copy state` root-count acquisition where another holder remains. At most 64 topic bytes per copy; no socket/task duplication. | Intended; app owner; drafts/check1. | 2 |
| Borrowed `mut Endpoint` / nested qualified Sender passed directly from mutable endpoint-list element. | CONC-6: current owned part lost sendable receiving evidence. | Separate enqueue helper gives the mutable endpoint its full contract. Envelope's frozen-root representation avoids relying on inferred deep uniqueness of enclosing state. No extra message queue. | Intended explicit seam; app owner; drafts/check2/check3. | 3 |
| Borrow-iterate `current.targets` or `endpoints[id].topics`, then mutate a deeply unique enclosing list. | Compiler bug: a borrowed field path was copied instead of lent (SEM-4 / MODE-2). | Borrow target/topic places directly; bounded survivor topic copies remain because the channel/helper contracts do not prove the endpoint owner unique for list.remove. | Bug, fixed by e5e974f27a4c; /tmp/pubsub-relay-30-check2.log. | 2 |
| Full app per-connection `scope_until` with captured borrowed Mutex ledger/status and owned socket/channel captures. | TIR validation before entry: `client.run` Body0 Block7 Inst5, ExpiredLoan loans1/2 Place58. Static check accepts. Minimal9 reproduces with two borrowed Mutex captures (Body0 Block6 Inst5 loans0/1 Place39). | Accepted source should lower consistently. Ordinary child scope plus supervisor's finite two-second check works; global serve still uses a real deadline scope. Supervisor checks on its 1ms timer; extra clock samples are bounded. Explicit helper/copies did not repair the original form. | Open compiler ownership/lowering defect; routed @agentlang-24. capture-take-reconcile-30 confirmed it predates fab899cd (parent 7acc69cf); owner is split-socket-loan-30. witness-loan/minimal9.hir, minimal7.hir control, original.log, pre-test-1.seal.json; original unbounded app runs sim5/sim6 are diagnostic controls only. | 18 |
| CLI `--simulation` file supplied `projections:[...,"monotonic_clock"]`. | `hirsel: simulation: unknown Io projection monotonic_clock`, exit3, before app execution. | Public Rust SimulationConfig supports the projection and clock grant; full app matrix uses that adapter. CLI JSON parser is narrower. No new runtime adapter was built. | Tooling interface limitation; app harness uses existing Rust adapter; /tmp/pubsub-relay-30-sim1.log and no-subscriber-simulation.json. | 1 |
| Scripted peer at Expect or Stall with a configured read timeout. | Current scripted providers return TimedOut immediately rather than parking. `io_net_script.rs:371-395`; same reference provider behavior. | Pending cancellation must be a genuine pending operation. Reader has unbounded socket read within a finite supervisor/global scope; write timeout stays 2s. `pending-read-cancel` runs through actual pending provider readiness, then joined cancellation. | Intended provider contract; application policy; 36-cell scripted receipts. | 2 |
| Attempt to treat scripted Stall as a blocked writer. | Current `io_net_script.rs:398-412` always accepts written bytes, with no write-timeout readiness state. | Script cannot prove socket backpressure. Keep blocked-write cells INCOMPLETE until an exact real Unix witness exercises them. No scripted Canceled error substitutes for pending work. | Provider limitation, not language failure; executable witness: witness-provider/main.hir, world.json and /tmp/pubsub-relay-30-provider.log. | 2 |
| First test check used `unwrap()` on ToolError. | Panic formatted the entire SourceMap (about 1.9MB), obscuring the two-line validator failure. | Test now prints Display diagnostics and retains its copied package before checking. No application or engine behavior changed. | Harness plumbing; app owner; /tmp/pubsub-relay-30-test1.log. | 1 |
| First setup/build seal did not re-evaluate stamp probes/globs or include installed compiler trees. | Review found stored stamp metadata only. | Extended external seal evaluates actual probes/glob membership, hashes live members and generated outputs, follows tool symlinks with cycle protection, and records CLI/image/tool bytes. Native harness seals link-kit/runtime archives before execution. Initial build remains honestly late/setup sealed, never relabeled. | Evidence setup; shepherd review; seal.py and *.seal.json. | 3 |

The first focused harness run selected one named test and failed during lowering,
before any workflow began. The corrected bounded application then selected one
named test and passed 18 complete workflows on reference and checked H (36 cells),
with fixed 10M fixture bounds declared before execution; actual consumed steps were
2933..430951, no tooling stops. The corpus/reference default remains 1M. Every
retained final state has no Stream, Half or Listener handles. These observations
are separate from native/replay and from the earlier diagnostic controls.

Production Ordered widths, native recordings, checked-host replay, exact broken
cells and remaining boundary obligations are listed with their actual disposition
in the external matrix. No source check/export or model-only test is an engine run.

## Native import blocker and final disposition

Both original native setups failed before executing any application instructions:
`native import: import instance 36: malformed: block 4 has no incoming state`,
exit 2 on LLVM and Cranelift. Exact sources, argv, matching CLI/image/runtime and
link-kit hashes and stderr are in `pubsub-native-jNDrUL/`. The shared importer
failure precedes backend choice. A standalone flow-module build is a successful
reduction control, not application acceptance. Owner: `native-import-state-30`.
The coordinator explicitly requested landing the interpreter app with the native
qualification test ignored and this owner named. The importer repair now leaves
arms removed by proven pattern tests un-emitted and prunes them before LIR
validation. The minimized witness is a string-payload or-pattern with a nested
match, from `wire.encode`; it needs no sockets or tasks. Native qualification is
re-enabled. No V/import check is disabled.

Bounded app coverage: 18 workflows + 11 boundaries on reference and checked H,
58 completed observations. Boundaries include topic64/65, data192/193,
frame2048/2049/zero, eight clients, eight topics plus one, 64 publishes, and
jobs1/4. Three actual Unix interpreter sessions (fanout/unsubscribe, domain
failure/continued service, partial-read cancellation) recorded immutable tapes.
All three replay through both reference and checked-H host adapters with original
socket providers and grants unavailable (six completed replays). Changed request
bytes and extra tape events refuse. Generic JSON reserialization of a config
proved only noncanonical-header refusal; a separate byte-preserving canonical
Args mutation proves binding refusal. Actual credit-finalizer vectors cover
four permits, one-over refusal, drop/reuse, two generic envelope payloads and
queued-disconnect loss on both reference and checked H.

**INCOMPLETE:** actual blocked socket writes, full-app slow/fast
reader backpressure, unsubscribe while partially reserved, sender disconnect
after partial reservation, and full-app pending cap4/one-over. The independent
model covers these decisions but is not a substitute for full-app observations.
Changed semantic-source replay refusal is also unrun. The app does not claim the
complete four-engine matrix passed. Original failures and unrun identities remain
in external matrix.json, actual-cells.json and remaining.json.

Harness setup repairs: production grants need explicit `io.dirs: []`; a missing
field refused before entry. Real-peer fixtures use short private temporary socket
directories and retain argv/grants/output in the evidence directory. Setup
failures remain in interpreter-host[-setup2]; successful host sessions are
interpreter-host-setup3. No budget or CPU/RSS cap was raised. Approximate active
effort: 3 minutes.

The per-connection supervisor uses a bounded 1ms virtual/host timer to enforce its
two-second maximum lifetime around genuinely pending reads; it is application
plumbing around the owned loan defect. A queued publish may be reported lost on
EOF or shutdown, including after local provider write failure. The report never
labels a socket write as remote consumption. Topic records are bounded for the
whole finite session, including inactive connection records.

Round 39 additive correction (`simulation-world-cli-39`): the historical
`monotonic_clock` projection rejection above is invalid JSON projection spelling,
not a missing clock implementation. The current interpreter projects clocks
from explicit `grants: ["Io", "MonotonicClock"]` and the `monotonic` value
(and `WallClock`/`wall` for wall time). Projection names do not create grants.
The CLI now transports the existing typed `signals`, `file_reads`, `file_faults`
and `mounts` worlds, plus exact binary `files` values. Fresh small pending
cancellation and commit-denial adapter identities and commands are documented in
[CLI simulation JSON](../../docs/native-test-fixtures.md#cli-simulation-json).
This corrects the historical explanation additively; it does not complete or
rebind Pubsub's 108 scripted capacity claims or the RE600 aggregate.
