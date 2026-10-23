# watchfiles

Wait for a bounded set of generated files, sampling their complete contents.
From the repository root, with a matching built CLI:

```sh
hirsel run --native --backend llvm examples/watchfiles/hirsel.json --grants examples/watchfiles/grants.json
```

The template watches `fixtures/ready.txt` twice. Its host directory is relative
_to the grants file_. For your artifacts, copy the template and set its `host`
to an absolute directory. Arguments are `INTERVAL_NS TICKS TIMEOUT_NS FILE...`;
for example `["100000000", "10", "2000000000", "build/a", "build/b"]`.
The interval must be positive, ticks 1..16, files 1..16, and each relative name
at most 1024 UTF-8 bytes, without empty, dot or parent components. No recursion.

The first tick starts immediately. Subsequent ticks wait one full interval
_after the previous tick's reads and output complete_. This fixed-delay policy
never catches up missed periods. Monotonic branded instants and checked addition
reject deadline overflow; clock errors propagate. Timeout zero expires before
any observation. The whole-run deadline interrupts idle waits and blocked reads.
Every nested tick joins its children before returning; at most four file tasks
are live, in consecutive batches. No platform watcher or persistent daemon runs.

Each completed tick emits tab-separated `tick`, `input index`, `status`, `bytes`,
in that order. Status is `initial` for a new/reappearing successful snapshot,
`present` for identical bytes, `changed` for different bytes (including equal-size
changes), `missing`, `error` for a file failure, or `limit` above 65536 bytes.
Missing/error/limit clears the comparison baseline. The next successful read is
`initial`. Contents are never printed. Failed ticks emit no partial rows; output
failure can leave a printed prefix. `complete` ends a successful run (exit 0);
`deadline` ends an expired run after cleanup (exit 1). Invalid input, grants,
clock and arithmetic errors return an application error (exit 1).

These are records of individual filesystem observations, never a simultaneous
snapshot of external state. A bounded read consumes at most 65537 bytes to
distinguish the cap from excess. Owned snapshots are kept for comparison, with
at most 16 × 65536 bytes of previous contents plus the current tick and scratch.

The application-private `watch.run` seam runs the identical scheduling,
batching, comparison and cancellation code with a factory of owned read jobs and
an ordered reporting callback. Permanent fixtures use virtual time and scripted
observations; real directory checks separately exercise `watch.read`.

Prior art: Tokio `ce685ee6916549ed7e52dbfd5430858bbdc734d3`,
`src/task/join_set.rs:381–393`, `tests/task_join_set.rs:267–294`,
`src/time/interval.rs:334–338`, `tests/time_interval.rs:35–136` (MIT).
Adaptations: Hirsel joins on scope exit; results use input order rather than
JoinSet completion order; all ticks use completion-relative delay; zero is an
application error rather than a panic; virtual time needs no granularity epsilon.
See `FRICTION.md` for diagnostics and validation limits.

The executable form snapshots trailing arguments explicitly:

```sh
hirsel build --backend llvm examples/watchfiles/hirsel.json -o /tmp/watchfiles
/tmp/watchfiles --hirsel-grants examples/watchfiles/grants.json 1000000 2 1000000000 ready.txt
```

The permanent CLI tests cover all twelve scripted workflows on H, checked H,
LLVM and Cranelift, compare complete deterministic native tapes at widths
1/2/4 (only admission time and placement provenance normalized), and exercise
real directory handshakes and cap/error controls. The blocked scripted reader
parks a provider response on a channel; it does not claim an OS regular-file
read was forced to stall. Host replay with files removed is covered by the
external qualification receipts; checked-H host replay is not available
through the current public adapter.

Recording-only production launches pass at observed widths 1/2/4. Combining
recording with `--trace-events` currently has an independently owned runtime
failure (`e1-trace-record-27`); these traced qualification cells remain open.
The example does not require tracing. No speedup is claimed for this timer/I/O
workload. Tokio's MIT notice is retained in `LICENSE-Tokio`.
