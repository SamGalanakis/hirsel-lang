# Qualification and friction

A useful bounded six-module application is implemented. Qualification is partial:
there is no all-engine claim for stopped, unrun or owner-blocked cells.
Evidence is retained under `/workspace/code/hirsel-briefs/discrete-simulation-33/`.
All original29 vector bytes, expectations and the1000000-step budget are unchanged.

| Cell family | Actual result |
| --- | --- |
| Reference and checked full main |68 semantic passes;14 original/supplemental Steps stops |
| LLVM and Cranelift full main |82 stdout/status/audit/semantic-stderr differential agreements |
| Native recordings |82 retained immutable tapes, including success/domain/denial/Pending cancellation |
| Native cross-engine replays |164 pass |
| Interpreter tape replays |136 pass;28 Steps stops, separately incomplete |
| Refusal controls |32 semantic controls;4 native extra-event returned-record obligations still open; raw refusal adaptation is separate |
| Native live stderr |40 nonempty-diagnostic obligations incomplete, shared owner native-file-faults-33 |
| Arithmetic/encoder helpers |2 helper tests on each native backend:4 passes; these do not replace full-app cap cases |
| Oracle |pinned SimPy vectors plus41 frozen tick-model cases; nonzero focused Buck execution |
| Ordered widths1/2/4 |18 success/domain/cancel cells unrun; exact-CPU deployment owner @agentlang-24 |

Four new nonignored Rust test registrations and two ignored Ordered registrations
were added (CLI inventory528 to534), plus two Hirsel helper tests. Focused actions
retained both failures and accepted subsets; no whole-matrix rerun for provenance.
Native main uses the public Fixture, SimulationConfig::from, differential_in,
link_fixture_runnable and launch_fixture paths. No corpus, std or public runtime
surface was changed. A dev dependency exposes the existing validated run record
for the ignored placement tests. No new language rule or provider was added.

The seven stopped vectors are over-jobs, never-admitted-events, cap-events,
cap-jobs, input-cap, cap-output-exact and over-output. Each has one reference and
one checked live stop, plus two tape-origin replay stops per evaluator. Native
oracle comparisons do not discharge those42 obligations. Output exactly65536
is a full-app native case;65537 is an encoder-bound helper, with full-app overflow
also retained. The helpers are additional evidence.

After the shared stderr fix lands, recover only the40 raw live cells, with a new
archival directory and no record/replay repetition:

```
.buck2/bin/buck2 test -c hirsel.tests=host //:hirsel-cli-test-cli-shard-1 -- --env QUEUESIM_RAW_STDERR_ONLY=1 --test-arg=discrete_simulation_33_native_full_entry_replay --test-arg=--nocapture
```

After exact-CPU deployment, recover the original capacity cells with:

```
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=discrete_simulation_33_ordered_ --test-arg=--ignored --test-arg=--nocapture
```

They require actual running control replies with requested=effective and pending0;
no topology override, CPU reservation, Free execution or speedup claim is used.
Missing replies remain failures. The public returned-record late-refusal witness,
its original empty returned field and raw refusal bytes remain separately open
under native-file-faults-33. Historical runtime-incompatible tapes stay incompatible.

Active implementation/diagnosis/documentation time: approximately100 minutes;
shared-pool queue time is excluded. Every adaptation and its cost follows.

- Initial parser spelling `heap` field and `let result`: LANG-14 reserved destination/postcondition names. Renamed pending/joined. Expected documented identifier rules; intended, no compiler bug. Zero runtime/copy cost; 2 active minutes.
- Initial `fn station`/`fn job` constructors plus same-named locals, and fn interruptible plus same-named local: LANG-4 no-shadowing. Renamed new_station/new_job/race_interrupt and inner queued_job index. Expected naming rule; intended. Zero runtime cost, 3 active minutes.
- Initial `events.Arrive`/Cancel/Release: LANG-4 unresolved constructor module spelling. Used contextual enum constructors. Expected sum-constructor resolution, intended; zero runtime cost, 1 minute.
- Initially plain String in Problem.Domain/Resource crossed task results: MODE-4 unsendable String. Arms now unique String; constants are owned leaf values. Expected owned sendable result; intended; one bounded owned diagnostic string, no scenario copy, 2 minutes.
- Initially plain mut output/station APIs weakened unique roots: private helper handback is fixed by private summaries (b552ff23d839 / f71ba1242e00); exported APIs keep their declared contracts. The `for` over station.queue sharing the root was a compiler bug, fixed by e5e974f27a4c (SEM-4 / MODE-2). FIFO removal now removes matching indices in place; no replacement list or survivor copies. Original workaround took 4 minutes.
- Initially arguments(args) uses{}: CAP-1 arg.len/text authority exceeds uses. Declared uses{args}. Expected explicit Args authority, intended; zero runtime cost, 1 minute.
- First plain check lacked std image and hit CPU cap with exit152. Build std image under normal setup limits; subsequent capped checks passed. Original stopped log retained; no resource cap raised; tooling setup, fixed, 3 active minutes.
- Supplemental helper tests initially used `test "..."` (LANG-1) then list.filled assigned directly to unique list (MODE-2). Used @test functions and explicit deep-unique copy in the test. Test-only64KiB copy, intended ownership cost, 2 minutes. Native10 canceled before app entry; original files retained.
- Native12 retention CAS raced between parallel test processes: hard_link AlreadyExists fell back to copy into the running binary's hardlink -> ETXTBSY. Fixed local harness publication with AlreadyExists success and temporary-file persist_noclobber fallback. No app/provider/runtime fault. Immutable artifacts retained by hardlink; cross-device fallback copies once per digest; 3 minutes.
- Native15/focus15 failed harness compilation E0433 because Ordered fixture code used hirsel_run_config without a direct test dependency. Added dev dependency and lock edge. No public runtime API/ABI changes; zero app runtime cost; 2 minutes. No tests ran in either action.
- Signal watcher formerly ignored an error from signal.wait and returned canceled. Tightened to Canceled(Requested) only for Ok(Interrupt), preserve scope Canceled(cause), otherwise signal_wait domain diagnostic. A denied-kind Pending-read full-main regression owns the new branch. Small typed error branch; no timer introduced; app fix, 2 minutes.
- Original interpreter cap/one-over vectors over-jobs, never-admitted-events, cap-events, cap-jobs and input-cap each stopped Steps at1000000 in both evaluators. Supplemental cap-output-exact and over-output also stopped there. Original bytes, expectations, fuel and source are retained. No raised budget or smaller replacement. Native-only oracle comparisons are separate identities; all14 evaluator obligations remain INCOMPLETE. Owner: simulator lane/evaluator budget, std codec/body costs; no claimed language bug.
- Ordered widths1/2/4 success/domain/cancel both backends remain INCOMPLETE, not inferred from Fixture agreement. @agentlang-24 confirmed exact CPU support is implemented but deployment awaits Sam. Public Fixture binds Args/mounts; re-linking public validated requested Placement changes only provenance. Tests retain required running/effective/pending assertions and complete scripted semantic events (only admitted_at/placement excluded). Free remains off. Rerun after deployment with ignored Ordered test names; no topology overrides or reservations.
- Queued native10/13/14 attempts canceled before application consumers; chronological source tar/seals and original logs retained. Native14 exact chronology was reconciled and accepted by shepherd650828; no mixed generation or execution credit. Native15 failed build; native12 failed before native app cells. Generation names and logs are distinct.

- Native16 exported/validated and emitted the real entry, but first fixture launch failed before app startup: `Failed to find executable : Is a directory`. Cleared launch environment left Python sys.executable unavailable through a bare command. Wrapper now uses absolute env/python3/prlimit plus explicit PATH. Source/API behavior unchanged, setup-only repair; no app cells credited to16. New consumer closure is sealed before resuming the same missing native identities. 2 active minutes.

- Native17 first reached genuine full-entry native execution: empty and FIFO differential LLVM/Cranelift passed, both empty tapes and the LLVM FIFO tape replayed through both native backends and both evaluators. Refusal request correctly exited3 and retained `stopped diverged ReplayMismatch(2)`, but the harness looked only at stderr; the documented Fixture observation record carries this diagnostic. Harness now uses public observe to classify refusal, preserves exact early-empty/late-full stdout requirements, and persists per-control receipts. Completed assertions before failure were appended to receipts with source/line evidence; they are not repeated. No runtime patch or weakened control; 3 active minutes.

- Native19 extra-event late refusal found the shared native-file-faults-33 seam: empty observation file wins over the emitted refusal artifact. Original public returned-record diagnostic remains INCOMPLETE. Immutable original/suffix tapes and raw0-byte observation/67-byte refusal plus SHA256 are retained in late-refusal-original; status3 recorded. The original failing attempt did not retain its buffered stdout, so its expected full prefix is labelled expected/unverified there. Local adaptation reads the emitted refusal through a copied Launched record for classification, while checking exact status and valid stdout bytes; no runtime/driver patch. Routed through shepherd651597; owner native-file-faults-33. Cost one bounded refusal-file read on negative controls only; 4 active minutes.
- Native18 compile-only E0308 from distinct Rust function-item types nested in a tuple array. Replaced tuple dispatch with a bool branch to the same two replay functions. No cell ran or expectation changed. Harness-only, fixed, 1 active minute.

- Native20 completed all12 small-success workflows, their24 immutable recordings/96 replays, all32 negative controls (extra-event diagnostics with labelled artifact adaptation), and domain differential on both backends. The first live domain recording then hit an incorrect host-stderr assertion: RealIo::fixture intentionally captures stderr in its snapshot/tape (launch.rs:2114), while replay echoes it. The exact domain stderr exists in the immutable tape and exit2/audit0 were already verified; classify that retained record without re-recording. Live host stderr must be empty; semantic tape stderr and replay host stderr still compare exact expected bytes. No codec/provider patch, zero app cost; 2 active minutes.

CORRECTION from shepherd651999: the live Fixture stderr omission is a shared defect owned by native-file-faults-33, not intended behavior. Tape PrintStderr equality is separate semantic evidence and does not discharge raw-live stderr. Original red, actualempty/expectedbytes, exit2 and tape/hash closure retained under raw-live-stderr-original. All nonempty-diagnostic live obligations remain INCOMPLETE pending owner SHA and missing-only checks. Native21 source remains frozen; no accepted recording/replay rerun.

- Final template review found a repository-relative host path, while grants-file hosts resolve relative to the grants file (run-config lib.rs:225). Changed it to `fixtures`; verified the resolved directory exists. No app source or accepted Fixture identity changed; zero runtime/copy cost, fixed, 1 active minute.
