# parhash authoring findings

Application-only work on current main; no ownership, runtime, task std or digest
rules are changed. Original failed drafts and receipts are retained outside the
repo in `/workspace/code/hirsel-briefs/parhash-27/`.

## Reserved local (about 1 minute; intended LANG-14)

The first check rejected `let result = task.await_at(s, mut pending, slot)`:
“`result` is reserved for the returned value in a postcondition; choose another
name”, at main.hir:70:13. Renamed the local to `joined`. Draft and full diagnostic:
`attempt-01/`. No runtime was executed and no checker repair is needed.

## Counted errors and task ownership (application plumbing)

The task set stores `Result[unique model.Entry, unique model.Problem]` and
its producer has that exact return annotation. Both typed records contain
String fields. The callback matches its ordinary helper result and explicitly
copies each complete payload with `unique`. This never relaxes sendability or
captures Scope into a child. Each child takes an independent path.

The first task check (attempt-03, about 2 minutes) reported CLO-3 at the
callback: `name` was not proved sendable despite its unique binding because
the borrowed helper call captured it by borrow. Taking `name` at that call
and declaring `take name: unique String` fixes the capture. The Entry moves
that owned name without an additional copy. This is intended capture policy.

The known kvd outer-unique-wrapper recipe source-checked, but full execution
checking rejected this app before runtime: two TIR TypeMismatch “value modes”
violations in hash, at the match following task.await_at. About 10 minutes
isolated the minimal form:

```hirsel
let mut pending: TaskSet[unique Result[String, String]] of s = task.set(s)
task.add(s, mut pending, task.async(s,
  fn() -> unique Result[String, String] { unique Ok("a") }))
let joined = task.await_at(s, mut pending, 0u64)
match take joined { Ok(row) => {}, Error(problem) => {} }
task.cancel_all(s, take pending)
```

The direct task.await twin exports; this TaskSet/await_at witness fails.
`Result[unique String, unique String]` in both set and callback, with
`Ok(unique "a")`, exports successfully. The app uses that payload-qualified
workaround. Complete minimal programs, exact errors and successful twin are
external `tir-mode-task-witness/`; owner routing is message637104 to agentlang-24,
for the existing uniqueness/task-result owner. No shared compiler repair.

Required explicit copies: JSON text into each manifest path; path into each
child; borrowed path and kind into a Problem on failure; complete Entry or
Problem at the task boundary. For N successful paths of total P UTF-8 bytes,
these sites can request 3P+64N bytes of counted string copies (P during decode,
P during capture, P+64N during result crossing), excluding JSON DOM, builder,
allocator and hex construction costs. This is a static byte cost, not measured
allocator counts. The decoder also requests an independent manifest String of M bytes. These are bounds at explicit ownership boundaries. A separate measured
companion below records actual whole-run physical copies. Failed
paths additionally copy kind/path for their Problem. All path copies are
bounded by 512 bytes and N by 64. No count-sharing rule is weakened.

## Bounded fan-in and failure (application policy)

Batches are the slot discipline. Completion-order draining discovers the first
completed error and explicitly cancels/joins remaining tasks. Indexed Option
slots retain manifest order, including duplicates, before a complete batch is
appended. No stdout is emitted until all batches succeed. Scope exit covers
language failures and cleanup. Reserving paths*byte_cap at admission bounds
payload without sharing a mutable total among children. One extra EOF probe per
file can be consumed. Metadata refuses every symlink component; metadata/open
remain separate and the root provider maintains confinement under replacement.

## Waiting regular-file cancellation (open acceptance cell)

Minimal required orchestration: one child in `file.read(d, mut f, mut bytes, 1)`
waits for a provider, while another child fails; drain must cancel/join it.
The public simulator offers File faults, including injected Canceled, but no
pending/readiness script for a regular file. A host FIFO is nonregular and this
app intentionally refuses it before open. Injecting Canceled would test error
mapping and would not prove cancellation of a waiting child. That cell remains
INCOMPLETE; no provider/runtime extension or substitute model is claimed.
Owner: parhash-27 acceptance follow-up; shared runtime changes require the seam
owner. SHA-256 itself is a bounded noncancellable compute grain.

## Setup limits (retained attempts)

Initial bundle setup selected std native objects unnecessarily and was canceled
by this lane after the matching CLI/image were ready; focused CLI/image build
succeeded. A 60-second capped source check then timed out because the wrong `HIRSEL_STD_IMAGE` variable triggered source
std precheck (734 MiB peak). This is a setup-limit cell, not a successful source
check or an application runtime result; its original log is attempt-02/check.log. Shepherd identified that this compiler
consumes `HIRSEL_IMAGE`; the corrected variable selected the matching built
image and reached application diagnostics in 1.1 seconds at the same cap.

## Runtime and qualification receipts

Source checking alone did not expose the TaskSet TIR mismatch; test-attempt-01
failed before executing workflows (0/5 tests). Test-attempt-02 passed both
reference/checked-H workflow partitions (36 app observations) and failed the
native canonical observer plus a combined six-vector primitive program that
exceeded its original 1M-step budget at std sha256.hir:362:42. Each unchanged
constituent vector and split-update assertion was then executed separately at
the same budget: 12 model observations passed. No budget was raised.

Canonical `--hirsel-trace-events` initially made even an empty app report exit
3 after correct stdout: `hirsel-rt: launch refused: incomplete canonical event
evidence`. All 24 original LLVM/Cranelift production trace cells are retained
externally in production-01. The event export seam belongs to e1-trace-record-27
(message637228); no duplicate runtime patch. Record-only production is separately
24/24 successful and never relabels the traced cells green.

The public native harness uses record-only execution and immutable tape replay
with the original mount unavailable. It checks all exact bytes/status and input
preservation; the supported replay adapter checks event order and full request
payloads, including unconsumed events. No trace sorting/filtering is used. Initial
wide replay wrappers accidentally imposed one CPU via tools/capped HIRSEL_JOBS=1;
the four timeouts are setup-constrained cells, not established replay defects.
Retrying only failed/missing cells with a fixed four-CPU wrapper yielded 138
additional greens; the 24 prior greens remain credited. These replay-only worker
hooks are separate from production recording, where all HIRSEL_RT_* were stripped.

The varied fixture proves completion indices [1,0,2,3] while output remains
[0,1,2,3] on reference and checked H; all four file handles are dropped. Logical
maximum outstanding jobs is asserted, including jobs1 and jobs8 with nine items.
Waiting-file cancellation stays incomplete as described above. Production
control snapshots report actual width separately; requested4 can initially be
effective2/3 while workers start and is never treated as proved4.

Timing-01 retained thirty setup-only taskset failures after G7 cores became
reserved; zero measured processes ran. Timing-02 stopped on its first setup
failure: nesting tools/capped inside timing.slice moved its child to an ordinary
user scope excluding the reserved CPUs. No ratios are inferred from setup.

## Measured copies and timing disposition

The public native emit/link adapters select the existing measurement archive;
no profiler/runtime extension was added. Initial `native::link` bound an empty
fixture grant, so the companion exited3 before output. Switching to the public
`link_standalone_objects` seam accepted fresh grants, then exposed a missing
trailing Args snapshot (usage error, exit2). Supplying the two real application
arguments fixed the companion. Original test-copy-01/02 failures are retained;
about 8 minutes including builds. These were harness setup errors.

Final measured duplicate-abc workload: 8 deep copies, 11 copied blocks,
726 copied bytes, 1 COW detach, peak logical live bytes1184 and live bytes0 at
exit. Counts cover parsing, paths, task crossing and output construction; they
do not attribute every byte to the workaround. Test-copy-03 passed1/1 through
the existing public native adapter, separately from production placement proof.

The final ordinary-CPU wall-time campaign used identical four64KiB files,
jobs4, byte_cap65536, LLVM executable and recorded policy for ten seeded
interleaved rounds per requested width. Recording ON, canonical observer OFF,
metrics enabled identically. Median wall times at requested1/2/4 were
30.99/33.94/35.77ms (ratios1.000/1.095/1.154). Compilation and control handshakes
were outside timing; process/perf startup was included. These short runs are
UNCERTIFIED: shared host, ordinary authorized CPUs after G7 closed, and no
per-run effective width samples. They establish no physical scaling or speedup.
Thirty earlier ordinary timing-03 usage-error runs are retained separately:
the wrapper omitted trailing application arguments, so they are invalid useful
work measurements. Timing-04 corrected only that setup and passed30/30 with
all immutable tapes, checksums and perf cycles/instructions retained externally.

Five production width samples initially missed the final requested width.
Qualification-02 could not start on reserved CPUs; qualification-03 retried only
those missing width cells on ordinary scoped CPUs and sampled the control
endpoint until the requested width appeared, without an artificial app wait.
All five now observe requested=effective1/4, complementing the prior1/2/4
samples; original startup0/2/3 samples remain unchanged. The canonical observer
failure is still owned independently.
