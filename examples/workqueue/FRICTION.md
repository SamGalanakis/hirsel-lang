# Workqueue friction and qualification

The application started at `ff3886a3b2e1dec2912efca90e4d995fa38f070a` in an
owned worktree. The original checkout's eight untracked paths were untouched.
All six sources are explicit manifest modules. Evidence and immutable drafts,
oracle, matrices, receipts, tapes and tool seals are external under
`/workspace/code/hirsel-briefs/durable-queue-31/`.

## Workarounds and dispositions

| Attempt / actual diagnostic or behavior | Expected contract / resolution and cost | Effort, disposition, owner, witness |
|---|---|---|
| `Finished(result)` rejected LANG-14: result is reserved | Rename to value; no runtime cost | ~2min, intended, application; draft01/tests02 |
| Locals pending/plan/sender shadow same-module definitions; group resolves as std module before dot | Distinct local names; no runtime cost | ~4min, intended, application; draft02/check03 |
| `record.Group` and `record.Delivery` unresolved | Variant constructors resolve from their expected type, without a module-qualified constructor spelling | ~2min, intended, application; check03 |
| Unique Envelope containing a fresh Sender rejected MODE-2; taken packet did not retain deep uniqueness | Send a record of Shared[Command] and Sender roots; uniquely copy command inside shared.with at the admission owner. At most1024 payload bytes copied per request. Unique Bytes replies move through their channel. Generic String/Bytes tables still consume unique payloads without copying T | ~8min, intended conservative boundary, application; draft02/check03–05 |
| Planned event accepted as plain taken Event did not prove unique Plan result | Declare the consumed event unique; no copy at construction | ~2min, intended, application; check03 |
| Snapshot child cannot capture parent Scope | Child opens its own scope for cancellation checks; freeze a bounded copied state for read-only preparation. At most128 payloads copied per checkpoint | ~3min, intended, application; draft02/check05 |
| Stop channel send is not completion of socket write | Worker sends Stopped only after net.write_all; owner then cancels/joins accept and all client children. One bounded control event, no polling | ~4min, ordinary application correctness; dispatch source and stop fixtures |
| File readiness release can precede read | Startup metadata operations allow loader to park before the starting marker releases AfterWrite. Both reference and native tapes assert an actual Blocked file read; cancellation uses Pending, never injected Canceled | ~4min, intended provider orchestration; scripted08/native19 |
| Initial harness used nonexistent ObservedSimulation.files (E0609); QueryError lacked Display (E0277) | Use trees[0].1; debug-format only query errors, display-format rich source errors. No application cost | ~4min, setup, application; tests01/06 |
| Lowercase pending/after_write fixture tags rejected before the next run | Public Rust enum uses Pending/AfterWrite. Corrected only missing partition;32 completed reference/H cells were retained | ~2min, setup, application; tests07/reference08 |
| Source-only check hit SIGXCPU before a verdict | Preserve exit152; consume shipped std image. The image-backed check passed in2s. No semantic cell was a tooling pass and no fixture bound was raised | ~3min, tooling setup; check04/05 |
| First standalone build omitted HIRSEL_LINK_KIT | Materialize matching CLI/std image/runtime archives/kit and set exact paths | ~4min, setup; buildllvm09/11 |
| Native fixture wrapper after env_clear reported Failed to find executable : Is a directory | Use absolute /usr/bin/python3 and explicit env, tools/capped, prlimit. All native fixture children use2GiB/60CPU-s and four-CPU wrapper allowance; requested widths belong to run placement | ~5min, setup; native10/11 |
| First seals copied the generation stamp without evaluating its probes/globs or exact tools; one capture included a shared mutable cache | Retain these as partial/diagnostic and honestly postlaunch. Later boundaries evaluate live probes and glob membership/member bytes plus BUILD_INPUTS/outputs/configs and exact compiler/image/runtime/LLVM/Rust trees, following directory symlinks with inode cycle protection. No green was repeated just to replace a seal | ~10min, evidence correction; boundaries01–05 onward |
| Native snapshot replay with Fixture.net absent failed UnixListen ReplayMismatch(14) | Preserve old and fresh matching-tool tapes. Existing owner supplied additive harness initialization repair a2621482; no tape matcher, runtime adapter or compiler change belongs to this app | ~15min active diagnosis/handoff, fixed by unix-listen-replay-31; native11/15/17 and owner's REPORT |
| Original pre-fix tape refused current compilation at program binding | Keep exact configuration mismatch as its own disposition. A coordinator-requested matching-tool stop recapture was separate; fabricated interim expected Observation / PASS_LIVE branch in native13 has zero new live credit and was removed | ~6min, setup/evidence correction; native13/15/17 |
| Extra-event refusal control expected empty stdout although refusal occurs at finish | Require exact original output prefix and exit3 for extra events, empty output for pre-entry controls. Preserve original assertion red; recover missing native suffix and remaining controls only | ~3min, setup; host-checked30/controls31 |
| Backpressure viewer refused a default timing sidecar under a different capture identity | Read the recorded sidecar's capture ID and inspect offline; retain the completed LLVM live tape and run only missing audit/replays. No live rerecord | ~2min, setup; pressure24/26 and LLVM retained replay receipts |
| A retained-harness copy/seal used a crate cwd for repository-relative paths | Original cp/seal failures are setup-only, zero observations. Copy/seal from explicit root; launch with absolute paths from the harness crate cwd | ~1min, setup; extra-checked33/34 |
| First crash helper redeclared event | Rename duplicate_event; the unchanged four-engine crash-cut workflow then passes | ~2min, intended, application; crash23/check24/crash27 |
| Interior length corruption looked like a short final payload and discarded following complete records | Real red: change message length12 to100, followed by a complete group frame; old service returned ready/joined and rewrote to magic. Validate available tag/embedded length against advertised length before accepting a short tail. No format change, extra allocation or std change. Three independent malformed-kind vectors reject without log mutation; six genuine torn-tail vectors still recover | ~10min, fixed application bug; length-witness.py, original red, draft-before-length-fix, tail25/native28 |
| Native Fixture has readiness scripts but no file-fault queue (SimulationConfig.faults does) | Exact append0/append3-byte-prefix/sync FileFault(7) main fixtures execute in reference/H. Their6 native fault-provider cells remain INCOMPLETE. Real permission-denied append after application-ready supplements them on both native backends; it is not relabeled as the scripted partial-write or sync cell. No incidental runtime adapter | ~3min, adapter limitation, existing runtime providers; boundaries.json and extra20 write-failure |
| 512KiB byte limit is dominated by other valid-state bounds | 128 maximum messages plus384 fixed-size delivery/group records fit below512KiB. Retain exact-size and one-over byte fixtures: exact-size malformed input is Corrupt, one-over is log_bytes refusal before open. Do not call the malformed exact-size cell a valid queue | ~2min, intended application bounds; reference21/native19 |

## Semantic and replay receipts

The independent dictionary state oracle and bitwise Castagnoli CRC are in the
external oracle.py. The CLI/schema, effects/error classes, per-fixture10M/30M
step policies and engine/provider matrix were fixed before implementation.
Default reference/corpus1M remains unchanged. Every completed observed run
retains requested/consumed step_usage separately from its semantic observation.
No Steps stop or zero-test build is counted as a semantic pass.

Application Hirsel:1194 lines: main80, record306, log134, groups301,
dispatch250, snapshot123. Tests, fixtures, grants, licenses and docs excluded.

Baseline:0 workqueue test identities and no application. This change adds7 Rust
identities (three explicit archival/manual campaigns). Source inventory was489
CLI tests at the original base;497 at the first matrix boundary, and485 after incoming main retired unrelated tests. Workqueue remains0→7.
The selected built harness reported500 identities before the replay-owner
registration and488 on the final rebuilt main; these are inventories, not whole-harness execution claims.

At the qualification boundary:

* 37 full-main workflows:74 distinct reference/checked-H cells pass.
* 34 available workflows:68 distinct native full-main cells pass. The6 exact
  native scripted-file-fault cells remain incomplete.
* Tail repair re-executed12 affected valid-tail cells in each two-engine
  partition; these are repair verification, not new distinct histories.
* Supplemental crash-cut Test entry passes reference, checked H, LLVM and
  Cranelift: synchronized message/group/claim/redelivery/ack/checkpoint,
  stale handles, and the last unsynchronized append across four crash seeds.
  It supplements, never substitutes for main-entry fixtures.
*18 ordinary production Ordered real Unix/file runs pass success/domain/cancel
  at requested1/2/4, logical jobs4. Every cell retained an application-ready
  handshake and running control reply effectiveN,pending0. These prove capacity,
  not simultaneous hardware execution. The later tail guard does not execute
  on their unchanged complete/missing-file startup paths.
*10 real native lease/restart/checkpoint-restart/permission-failure runs pass;
  two actual blocked-write backpressure runs cancel and join cleanly.
* Each ordinary production recording is replayed without original queue files,
  grants or Unix peers. Original stdout/exit and immutable tape bytes match.
  Independently sampled live clocks are compared to their own tape only.

Scripted success/domain/cancel width1/2/4 adds18 native captures and72
replays, with12 full Recording comparisons against width1. Only admitted_at
and placement are normalized; task IDs, errors, clocks and event order stay
exact. One wrong width-baseline path stopped after a completed live capture;
its real serialized observation/tape was resumed with zero new live credit.
The current owner-repaired stop recapture passes both backends plus8 replays.
Original real workflow tapes have120 replays across the four engines (30 each);
16 distinct input/config/missing/extra-event refusal controls pass. Tape audits
check79 original captures for final bytes and sync-before-reply.

Precise final replay/width and archival-audit counts are retained in the
external REPORT and per-cell ledger; additional witness or repair runs are
not folded into distinct functional coverage. Snapshot replay is separately
identified from ordinary executable replay. Old snapshot failures and the
configuration refusal remain preserved even when the owner repair passes.

## Reference deviations

Redis7.2 BSD pending/ack vectors retain independent per-group history and
consumer ownership, with integer group/consumer IDs and bounded vectors in place
of names/radix trees. Tokens intentionally strengthen XACK. Restart expires
process-relative leases. There is no trimming, replication or exactly-once claim.
LevelDB BSD FULL headers/checksum/recovery concepts use a private bounded stream:
no32KiB fragmentation/padding, no skip-and-continue after corruption. A short
payload must agree with its available type/length prefix. The checkpoint is one
synced replacement log rather than a second-file protocol. Shipped crc32c and
binary/file/task operations are reused. LICENSE.redis and LICENSE.leveldb retain
upstream terms; localrpc's Tokio-derived fixed-length framing terms accompany
README.md.

The final missing-only width2/4 comparisons pass; wrong baseline lookup and
helper label-scope E0425 build failures remain setup-only. CONTROL_CELLS=none
used to print an unconditional all4 label; those labels have zero control credit.
The helper now reports only actually executed adapters. controls31 supplies the
refusal proofs. The historical snapshot replay reds/unrun identities stay in the
ledger; ordinary production replay supplies their native semantic counterparts.
The owner-repaired matching stop cell is closed, not left assigned to a settled
lane. The six exact scripted native file-fault cells remain INCOMPLETE.
