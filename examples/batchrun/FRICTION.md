# batchrun findings

Initial implementation pin: `7f58de80430fce90f9a9981ab98f04a7a961418d`.
Immutable drafts, diagnostics, seals and per-attempt logs are retained externally
under `hirsel-briefs/batchrun-27/`. No language/std/runtime repair is included.

1. **Variant constructors (intended syntax; about 3 minutes).** Attempt 01 used
   `model.Timeout()` and other module-qualified arms. Check reported LANG-4,
   `unresolved name model.Timeout` (main.hir:19:19), and the other qualified
   variants. Hirsel has no qualified variant constructor: the return/argument
   type supplies the nominal context. The repair uses `Timeout()` with an
   explicit `model.Outcome` result. Minimal: `fn f() -> model.Outcome {
   model.Timeout() }`. No copies or runtime cost.
2. **Tuple prefix typing (intended; about 2 minutes).** The same draft used
   `let (status, code, signal, timeout) = match outcome { ... None() ... }`.
   LANG-2 at model.hir:56:34 and adjacent branches said generic call arguments
   cannot be inferred from prefix and expected type. An explicit tuple type
   `(String, Option[i32], Option[i32], bool)` supplies the missing context.
   No compiler change or runtime cost.
3. **Consuming a bounded result (intended ownership; about 2 minutes).** Attempt
   02 matched `bounded` by borrow then returned `take statuses` from its arms.
   OWN-1 at main.hir:79:27: “a borrowed value cannot be taken: bounded is local
   to this function, but this needs an owned value”. Repair: `match take
   bounded`. The original owned list moves; no additional deep copy.
4. **Owned command and capability capture (documented plumbing; about 15
   minutes, selected by inspection).** Parsed strings are copied with `unique`
   into the admitted job list. `model.command` copies each argv tail string into
   a fresh list and returns `unique Command`; the child closure takes it. Its
   program, args, directory/environment fields and `own_group` are explicit.
   Proc, clock and Tasks handles cross by their documented capability rules.
   A child obtains a fresh Scope via captured Tasks; it never captures a parent
   Scope. The cost is two bounded argv-string copies across parse/admission and
   per-spawn construction (at most 8 KiB each); no mutable closure exception,
   Command equality assumption or task-result uniqueness relaxation.
5. **Typed status conversion (ordinary application plumbing; about 10
   minutes).** ProcessError becomes a plain domain enum before crossing the
   task boundary. AppError wraps OutputError/json.JsonError for `?`. These
   declarations keep failures reportable without making a nonzero exit cancel
   the batch. No failed diagnostic is claimed for these design choices.
6. **Both pipes and child cleanup (documented recipe; about 10 minutes).**
   `child.wait` closes stdin, concurrently drains/discards stdout/stderr and
   consumes/reaps the child on every outcome. Per-child and whole-run scopes
   provide cancellation and join. Reports are built only after joins; bounded
   result slots retain manifest order. This intentionally does not capture
   output. `own_group:false` restricts management to the direct child.
7. **Harness setup errors (not language defects; about 15 minutes).** First
   focused attempt: simulation lacked the MonotonicClock root grant; native
   record flags after grants were application args (`kind:usage`, exit2);
   interpreter recording inside its granted mount was refused with
   `kind:recorder_overlap,mount:fixtures`, exit3. Attempt 02 repaired these,
   but put the trace option after recording, yielding native launch refusal
   `field:argv`. Trace must be the first observer prefix, then recording, then
   grants. Original logs remain; successful cells were not rerun just to obtain
   a green wrapper.
8. **Ordered tracing defect (runtime, routed; about 15 minutes to minimize).**
   Correctly ordered native trace/record flags made even the empty batch print
   its expected summary then abort (exit134), both backends, width1, with
   `io_home.rs:48`, `ordered_home_results.insert(task,snapshot).is_none()`.
   Minimal external `minimal-01/scope-clock/main.hir`: project stdout/Tasks/
   monotonic clock, sample it, run an empty `task.scope_until`, then print.
   Ordinary recording succeeds; adding `--hirsel-trace-events` aborts.
   One/two-print traced controls succeed. Owner: `e1-trace-record-27`, assigned
   by @agentlang-24 after the earlier Ordered owner had settled. Application
   tests use ordinary recordings; the failed traced cells remain failed until
   an owner repair is independently qualified. No second repair is included.
9. **Deadline versus fixture startup (expected host ordering; about 5
   minutes).** First Cranelift global-deadline run had zero `.started` receipts
   when its 300ms deadline fired. Cancellation may prevent a child from running
   any fixture code, so 0..jobs handshake PIDs is legal. Every observed PID
   must still be absent from `/proc` after return (including zombies), and
   queued jobs must not run. Positive multi-child admission/liveness uses a
   separate hold/release handshake and live control snapshots. The failed
   assertion requiring exactly two PID files is retained; the deadline, process
   caps, expected global_timeout output and no-survivor checks are unchanged.
10. **Interpreter argv boundary (OPEN tooling limit; about 5 minutes).** The
    complete app with one argv of exactly 8192 bytes, inclusive of NUL
    terminators, reaches `ToolingLimit{kind:Steps,limit:1000000}` at
    `std@1/substring.hir:129:28`. The unchanged vector is
    `tests/support/batchrun_27/caps.json`, identity `argv-8192`. Native tests
    retain the full admission/command/output assertion. Interpreter acceptance
    at this boundary is INCOMPLETE; checked-H boundary execution was not
    reached. No limit is raised, std optimization attempted, or smaller
    substitute credited as the full boundary. Owner: batchrun follow-up.
11. **Test support slicing (harness defect; about 3 minutes).** The first cap
    partition sliced argv `[1..]` while constructing scripts for an intentionally
    empty argv, panicking in Rust before that workflow ran. `get(1..)` returns
    an empty script tail for this invalid manifest. The earlier individual
    greens remain credited; only the missing partition is resumed.
12. **Periodic metrics on a short run (tooling; about 3 minutes).** A successful
    qualification run exited before periodic metrics wrote a JSON line. Parsing
    its empty metrics file failed after the live control reply had already
    proved width1 and the child/output/tape checks had completed. That live
    launch was recovered from its retained stdout/control/PID/tape evidence,
    then only its missing replay work and the remaining cells were run.
    Empty periodic files are retained as empty; control replies are the width
    observations, with no invented periodic sample.

Reference deviations: Tokio abort-all/join structure becomes core scope
cancellation plus task-set joins; results retain command order, failures become
statuses, and admission refills one slot per completion. Go's copying-loop join
becomes Hirsel's documented discard-only `child.wait`; test output is bounded to
96 KiB on each stream, stdin is closed immediately, and no shell or descendants
are created. Deterministic scripts use tiny payloads for semantic equality;
real child fixtures establish host pipe backpressure/EOF and reaping. Scripted
children have no timed step or stdin-EOF wait, so those script cells are not
credited as real deadline/EOF evidence. The tool introduces no new concurrency
construct or public combinator.
