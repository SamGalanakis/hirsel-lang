# firstreply authoring and qualification friction

Application base: `7f58de80430fce90f9a9981ab98f04a7a961418d`.
No language, std, runtime, parser or checker implementation changed. Approximate
minutes below are active investigation, not performance measurements. Original
drafts and exact diagnostics are retained externally in the firstreply-27 lane.

## Explicit owned captures (about 2 minutes; intended CLO-3)

The first checked draft created independent copies but borrowed them in the child:

```hirsel
let owned_payload = unique payload
let owned_request = unique sent
let h = task.async(s, fn() -> unique Result[Bytes, QueryError] {
  unique exchange(n, socket, owned_payload, owned_request)
})
```

The actual check reported CLO-3 for each capture, twice through verification:
“task captures must copy or take sendable values; Receivers must be taken:
`owned_payload` is not proved sendable, but the closure crosses into a task”,
and the identical message naming `owned_request`. Its help says “Capture owned
sendable data with `take x` at its last use”. Source: `main.hir:56:48`, original
byte span 2156..2275. The corrected expression is
`unique exchange(n, socket, take owned_payload, take owned_request)`.
The two copies already existed; the correction adds moves, not copies. Source
check then passed. Each child copies at most 2048 request bytes and one bounded
parsed Ping/Normalize value, and produces an owned Result. The outer `unique`
result may additionally copy its bounded reply/error. This is the known
sendability recipe, not a new language defect. No scope is captured into a child.

The planner's counted-error MODE-4 and receive-context MODE-2 witnesses were
rechecked at current main: the intended rejections remain, and the owned-error
repair, map and credit controls accept. They are controls, not newly found bugs.
No channel is needed here, so no receive-context workaround was introduced.

## Harness and launch plumbing (about 8 minutes; application/test setup)

The first focused harness started two tests and failed both before application
execution: the examples directory is not automatically runtime data in Buck's
sandbox. It now materializes `include_str!` sources/configuration, following
localrpc's existing harness. The next build exposed stale generated include
inventory; `tools/buck2/sync.py --if-stale` regenerated it. Neither required a
compiler change.

`SimulationConfig` requires both the `monotonic_clock` projection and the
`MonotonicClock` grant. Omitting the latter rendered `{"error":"io"}` in the
first simulated attempt. Adding the explicit grant made all ten complete app
workflows pass under the interpreter and checked H, including full ordered
effect equality. This was a provider setup error.

A standalone `--hirsel-grants FILE` snapshots the arguments directly following
FILE, replacing the template's `args`. Omitting them yielded usage; inserting
`--` made it the first application argument and also yielded usage. Pass
`TIMEOUT_MS REQUEST SOCKET...` immediately after FILE. The CLI's `hirsel run
--grants FILE` reads the template's `args` instead. These exact failed attempts
are retained; the README spells out both forms. No launch semantics changed.

## Native replay mismatch (open; runtime owner e1-trace-record-27)

The application executed all ten scripted cases on both native backends and all
four native real-peer cases correctly. Only the two `success-before-pending`
native tapes fail interpreter replay with exactly
`hirsel: replay refused: ReplayMismatch(12)` (status 3).

The reduced program races a child that connects, writes `b"ping"` and reads one
byte against a child connecting to an `Unanswered()` scripted Unix peer, under
`task.select` and an absolute deadline. Native record returns normally; replay
refuses. Removing the write makes the frozen twin replay successfully. Both
backend application tapes, the smaller source, grants, exact argv and hashes are
retained in external evidence. The owner received the differential; this lane
makes no second runtime repair. The permanent CLI test retains this failing
replay cell and has an explicit known-failures row. No first-success policy or
extra delay is used to evade it. About 12 minutes were spent isolating/routing it.

## Canonical native trace export (open; same owner, distinct witness)

A minimal `main(rt: Io, args: Args) -> process.ExitCode` that calls `arg.len`,
prints usage and returns status 2 instead exits 3 when both recording and
`--hirsel-trace-events` are enabled. Exact stderr:
`hirsel-rt: launch refused: incomplete canonical event evidence`.
Removing Args/arg.len makes trace export succeed. Both tiny sources and the
failed tape are retained and routed separately from the replay mismatch.
Untraced recordings remain enabled for qualification and timing, as the owner
instructed. No event trace is sorted, filtered, or presented as complete.
About 5 minutes were spent reducing this witness.

## Evidence limits

Live-clock recordings with scripted Net are live inputs: their independent clock
samples need not match across widths. The real-peer production cells use control
handshakes to observe effective widths; each immutable tape is replayed with all
peers gone. This is separate from replay-only topology hooks. Checked H's public
simulated-world adapter is used for the full scripted app; live host socket and
host-tape replay cells lack that adapter and are explicitly INCOMPLETE. No model
is substituted for them. Native deterministic-clock tape equality across widths
is also INCOMPLETE; independently sampled host clocks are never normalized away.

Initial remote fixture tapes were left in an ephemeral action sandbox; their
passing per-cell receipts survive in the original log, but those raw originals
were not recovered. The two failing tapes were reproduced locally once and
retained immutably. Support now returns compressed exact small tapes/receipts in
the test log, so future remote action cleanup does not lose them. Setup/check
seals that were late are labelled retrospective in the external report.
