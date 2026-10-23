# rpcbatch authoring and execution friction

First implementation base: `7f58de80`. Full failed drafts, diagnostics, peer
histories and immutable attempts are retained externally under
`/workspace/code/hirsel-briefs/rpcbatch-27/`. Approximate active effort below is
specific to this application. No compiler, runtime, std or language rule was
changed by this lane.

- **Reserved local name, intentional LANG-14 (under one minute).** First check
  used `let result = match deadline(c, millis) { ... }`. It reported LANG-14 at
  `main.hir:45:7`: “`result` is reserved for the returned value in a
  postcondition; choose another name”. Rename to `outcome`. The first draft and
  exact JSON diagnostic are retained in `attempts/check-01/`. No copy cost.
- **Owned outcome matching, intentional OWN-1 (about two minutes).** Check two
  used `match outcome { Ok(payload) => ("ok", Some(take payload)), ... }`.
  Diagnostic at `main.hir:53:27`: “a borrowed value cannot be taken:
  `outcome` is local to this function, but this needs an owned value”. The
  related `take response` obligation was folded into the same diagnostic.
  Repair: `match take outcome`. It consumes the result; no deep copy is needed.
  `attempts/check-02/` retains the source. Check three passed with zero errors.
- **Child ownership and domain outcomes, ordinary application plumbing.** Each
  child owns a unique Job and socket path, keeps the same captured Net brand,
  and returns `unique Result[Outcome, ()]` with its domain error inside `Ok`.
  `task.all` would cancel siblings on an outer Error. No failed alternative is
  claimed. Moving a pending Option out with `replace` preserves ownership;
  `unique job` copies at most one 2048-byte payload plus its bounded parsed
  fields. `unique socket` copies the path per child. The borrowed localrpc
  parser already makes bounded unique UTF-8/field copies documented in its
  FRICTION file. Unique final outcome construction copies bounded ID/response
  data to establish the task boundary. A wave barrier bounds child metadata
  and active connections by slots, at the cost of idle slots behind a slow
  member. This is an explicit scheduling policy, not a new runtime combinator.
- **Scripted idle accept, intended provider limit (about three minutes).**
  Minimal source is `task.scope_until(tasks, deadline, fn(s: Scope) {
  let mut listener = net.unix_listen(n, socket)?;
  net.accept(n, mut listener) })` with no scripted incoming clients. The
  documented `net.script@2` provider returns `ConnectionAborted` after its last
  client instead of waiting for an unknown external client. This cannot test
  expiry of a pending accept. The exact checked-H workflow remains INCOMPLETE;
  interpreter, LLVM and Cranelift real-socket idle accept runs test scope expiry
  and listener removal. Owner: application/provider policy, open coverage gap;
  no provider semantic change or replacement model pass.
- **Canonical trace export, open runtime defect (about five minutes to
  isolate/route).** Normal production LLVM Ordered width one with empty
  `batch SOCKET 4 100` prints `[]`, then exits 3 with
  `hirsel-rt: launch refused: incomplete canonical event evidence` when prefixed
  with `--hirsel-trace-events FILE --hirsel-record TAPE`. Its unchanged source,
  exact argv, grants and tape are in `native-N2GFt4/receipts/llvm-empty.*`.
  Recording-only execution and replay pass. Owner `e1-trace-record-27`,
  confirmed by orchestration messages 637010/637025 and owner reply; no second
  runtime repair. Production trace qualification is blocked on that lane.
- **Launcher prefix order, tooling recipe (about three minutes).** The first
  native fixture incorrectly put `--hirsel-record` after `--hirsel-grants`:
  those tokens became application arguments and produced `usage`, exit 2.
  The next draft put trace after record and was refused before entry with
  `Invalid(argv)`. Correct order is trace, record, grants, application Args.
  Original setup reds are `native-01` and `native-02`; no runtime failure is
  inferred from these draft commands.
- **Harness setup and independent oracle corrections (about five minutes).**
  Filtered CLI tests belong on shard 1: the initial shard-2 wrapper ran zero
  bodies. The first pool setup omitted localrpc's dependency sources and got
  ENOENT before execution; package setup now materializes the unchanged source
  from `include_str!`. The next simulated setup omitted the MonotonicClock root
  grant and returned `io`, exit 3; adding the explicit grant repaired the
  fixture. An oracle incorrectly expected `please Straße 羊`; ASCII lowercase
  must also change the `S`, yielding `please straße 羊`. Only the failed
  partition and its unexecuted remainder were rerun, retaining all prior greens.
- **Peer half-close versus full close (about three minutes; fixture fixed).**
  The first mixed production peer treated `recv()==EOF` as complete client
  cleanup, but the client intentionally calls `shutdown(Write)` before reading
  its reply. The peer therefore closed a stalled connection prematurely;
  actual status was `truncated_header`, not expected `deadline`.
  `production-01` retains the original script, tape and failing row. Waiting
  for POLLHUP instead preserves the socket until full close. Missing-only
  `production-02` retained all prior success/replay greens. All 18 production
  recordings and 126 replays then passed unchanged domain expectations. The
  permanent reverse-response peer also waits for full close after each reply,
  proving reversed completion before releasing the next response; only its
  affected three live cells and six replay cells were rerun (9/9 pass).
- **Short-run metrics, tooling limit (about two minutes).** The first timing
  process finished successfully with correct output and valid perf data before
  periodic metrics published a snapshot; its metrics file was empty. The first
  parser raised JSONDecodeError after that measurement. It was retained and
  recovered from the existing perf/output/tape files, not rerun. Other short
  timing rows tolerate absent metrics and report effective width as unobserved.
  Independent live qualification handshakes observed effective widths 1/2/4;
  they do not fill the missing per-timing-row observation. Timings are
  UNCERTIFIED and show no scaling benefit. No artificial wait was added to
  timed work. Owner: application evidence, open timing-width follow-up.

The known planner MODE-4 unique-result and MODE-2 receiver invariance examples
are diagnostic controls, not new defects in this application. The production
trace issue above remains a real open cell; source check, replay success or a
pure helper-model run does not turn it green.
