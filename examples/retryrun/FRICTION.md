# retryrun findings

Launch source: frozen origin/main e4f48f7a58ba6df14ef25b6b2079784770d8c6c2.
Raw attempts, JSON diagnostics, source hashes and command logs live externally
under `hirsel-briefs/retryrun-23/attempt-*`; they are not application fixtures.
No checker or standard-library changes are included.

1. **Reserved local `result` (intentional rule, about 2 minutes).** The first
   complete source used `Finished(result) => result` and `let result =
   schedule.retry(...)`. `check --json` reported LANG-14 at main.hir:36:14–20,
   bytes 2117–2123: “`result` is reserved for the returned value in a
   postcondition; choose another name”. The working repair renames both locals
   `retried`. Minimal form: `fn f() { let result = 1u64 }`.
2. **Variant obscures core constructor (intentional rule, about 2 minutes).**
   `type AttemptError = ... | Deadline` made `Deadline(clock: c, at: at)` fail
   LANG-2, “bare variant needs an unambiguous expected type”, at the two core
   constructor calls (attempt-02 main.hir:26:33–41 and 29:42–50). Minimal form:
   `type E = Deadline; fn f(c: MonotonicClock, at: Instant of c) { let d =
   Deadline(clock: c, at: at) }`. Renaming the application arm to
   `AttemptTimeout` restores the documented core constructor. No name-resolution
   change is requested.
3. **Scoped callable state (intentional rule; selected from docs, no failed
   diagnostic; about 5 minutes).** CLO-1 forbids assigning captured outer
   `let mut` storage. The operation needs a running attempt number, so it
   captures an Atomic handle and uses `atomic.load/store` with explicit Tasks.
   This adds a counter capability and Tasks plumbing; no mutable-capture
   exception is assumed. The CLI is sequential, so no atomic read/modify/write
   concurrency guarantee is needed.
4. **Child lifetime and two pipes (documented recipe; about 5 minutes).**
   Reading stdout alone, as the nearest `testrun` example does, could block on
   stderr. The runner uses `child.wait` to drain/discard both streams concurrently
   and uses both a scope and child deadline. It intentionally discards output
   rather than accumulating it. `max_elapsed` is used only between attempts.
5. **Pure/I/O seam and owned command arguments (documented rules; about 5
   minutes).** `model.command` copies each borrowed argv tail string with
   `unique` into a new bounded list before constructing the owned `Command`.
   Model admission similarly copies borrowed parsed strings and error-kind
   strings. Named `number`, `invalid`, `retryable`, `command`, `policy` and
   `filename` helpers keep validation/transformation pure. These helpers and
   copies are explicit application plumbing, not hidden compiler repairs.
6. **Typed wrappers and reports (documented rules; about 5 minutes).**
   `AttemptError` wraps ProcessError, ClockError and OutputError so `?` can inject
   each nominal error, while Exit/Signal/AttemptTimeout classify application
   outcomes. `AppError` wraps output/JSON errors. Reports use fixed status
   strings instead of runtime/debug rendering. The invalid record has a named
   `InvalidInput` type with a generated `derive.json_value` renderer. Numeric
   list literals and the Atomic seed have explicit scalar suffixes as required
   by prefix typing. `std@1/process.ExitCode` is fully qualified to select the
   documented entry-exit API.

The first model draft spelled JSON's text arm `String`; source inspection of
`json.Value` corrected it to `Text` before a model diagnostic was emitted.
No diagnostic is claimed for that edit. The initial first error was retained
before either this correction or the reserved-local repair.

7. **Policy-fixture reserved local (intentional rule; about 2 minutes).**
   The initial policy fixture used `let once = schedule.retry(...)`.
   Exported JSON diagnostic LANG-14 at policy main.hir:24:7–11, bytes
   1271–1275: “`once` is reserved for a type qualifier; choose another name”.
   The repair is `single`. Minimal form: `fn f() { let once = 1u64 }`.
8. **Whole Command equality (intentional bound; about 3 minutes).**
   `model.command(record) == expected` failed GEN-1, “equality requires data
   operands”, policy main.hir:38:17–50, bytes 2550–2583. Command can contain a
   `Dir` capability, so the test instead checks `program`, `args`, `own_group`
   and absence of `dir`/`env` individually. It preserves the same intended
   assertion without claiming whole-Command data equality.
9. **Helper/local name collision (intentional rule; about 1 minute).**
   Adding `fn at(...)` collided with the earlier vector's `Ok(at)` binding.
   LANG-4 at policy main.hir:4:45–47, bytes 324–326, “a local cannot shadow a
   top-level value in the same module”. The repair names the helper
   `assert_ticks`. Minimal form: `fn at() {} fn f() { let at = 1u64 }`.
10. **Grants template base directory (documentation usage; about 2 minutes).**
    The first shipped launch used `host: "examples/retryrun/fixtures"` in a
    grants file already under `examples/retryrun`. The launcher rejected it
    with exit 3, field `io.dirs.host`, detail
    `examples/retryrun/examples/retryrun/fixtures: No such file or directory`.
    The repair uses `host: "fixtures"`, relative to that template. This is a
    launcher refusal, not a compiler diagnostic. Tests use absolute temporary
    host paths when relocating grants records.

The first remote focused run selected two tests: the complete application
matrix passed (53 cases × 3 engines = 159 executions); the policy fixture failed
before execution on reserved `once`. The whole original generation and logs
are retained externally. The shipped probe was then changed from `/usr/bin/true`
to a realistic Python/SQLite dependency check. Only its two affected workflows
and template checks are executed separately, together with the repaired policy
fixture; the accepted child/pipe/grant matrix is not repeated. The application
main/model bytes are unchanged after that accepted matrix.

Final affected checks: `retryrun_23_shipped_workflows_and_template` passed two
shipped cases on H/LLVM/Cranelift (six executions), including the checked-in
relative directory grant, unchanged input hashes and temporary-directory cleanup.
`retryrun_23_virtual_clock_vectors_checked_h` passed 24 upstream delay values,
max_attempts(1), strict elapsed equality and its adjacent accepted value, time
inside the operation, plus pure model admission/command/policy checks on both H
and checked-H. Exact virtual timestamps are asserted. Source check and reachable
TIR exports of the application and policy path passed. Three distinct Rust tests
were executed and passed across the original and affected runs; the original
failed policy attempt remains a failed attempt, not a passed cell. No shared
compiler/std repair or routed issue was needed.
This inventory covers the enumerated scenarios; it makes no claim of universal
ergonomic coverage.
