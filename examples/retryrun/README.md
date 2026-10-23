# retryrun

Retry one explicitly granted command after transient exit codes. For an agent,
this makes a local cache refresh or build probe repeatable: each attempt produces
one JSON line, followed by a terminal record. No shell evaluates the argv.

From a fresh checkout with `hirsel` on PATH, run the small successful probe:

```sh
hirsel check examples/retryrun/hirsel.json
hirsel run examples/retryrun/hirsel.json --grants examples/retryrun/grants.json
hirsel run --native --backend llvm examples/retryrun/hirsel.json --grants examples/retryrun/grants.json
hirsel run --native --backend cranelift examples/retryrun/hirsel.json --grants examples/retryrun/grants.json
```

`success.json` checks that Python’s SQLite dependency can open, query and close
an in-memory database, a small dependency probe an agent can run before using a
local cache. It executes `/usr/bin/python3` directly, with this output and exit 0:

```json
{"attempt":1,"status":"success"}
{"attempts":1,"terminal":"success"}
```

Copy the grants file and change its `args` to `["invalid.json"]` for an invalid
retry budget: stdout is empty, stderr is `{"kind":"max_attempts"}` and exit is 2.
Set `args` to `["--help"]` (or `["-h"]`) for usage; only stdout/stderr grants are
needed. Relative host paths resolve beside the grants template; its `host: "fixtures"`
selects this package’s fixture directory. See the
[shared grants-base recipe](../../docs/native-verification.md#resolve-directory-hosts-beside-the-grants-file)
for original host/base/candidate diagnostics and verification from another directory.

The input object has exactly seven fields:

```json
{"argv":["/absolute/program","arg"],"retryable":[75],"max_attempts":3,"base_ns":500000,"cap_ns":5000000,"max_elapsed_ns":30000000000,"timeout_ms":1000}
```

The first argv element must be absolute and exactly match a process rule. Program
arguments are literal; environment is empty unless explicitly permitted by a
process rule (the shipped rule permits none). Children start in `/` and in their
own process group. A process grant permits arbitrary arguments to that executable;
review the command record as well as the executable grant. The runner itself
requests only read access to `fixtures` and writes only its granted output streams.
A child has the host authority of that executable; its effects are not confined by
the runner's directory grant.

Admission bounds are one record, 8192 input bytes, exactly seven fields, 1–16 argv
elements of at most 256 UTF-8 bytes each without NUL, and at most eight retryable
codes in 1–255. Input is bounded before JSON parsing; transformed collections are
checked before appending. JSON's shipped nesting bound also applies. Files must be
single filenames (at most 256 bytes, without slash, backslash, colon or NUL).
`max_attempts` is 1–5, `base_ns` and `cap_ns` are 0–20,000,000,
`max_elapsed_ns` is 1–30,000,000,000 and `timeout_ms` is 1–5000. With at most four
waits and a 20 ms cap, total requested delay is at most 80 ms. Host scheduling may
overshoot a wait. Attempts include the initial call and failed spawns.

The policy is `max_attempts(max_elapsed(capped(exponential(base_ns, 2), cap_ns),
max_elapsed_ns), max_attempts)`. Clock and random capabilities are explicit. Jitter
is omitted, so output and requested delays do not depend on entropy. Elapsed time
includes child execution. Strict equality stops before sleeping. This elapsed
budget does **not** cancel an operation: each child has a separate monotonic
deadline, enforced by `task.scope_until` and `child.wait`. Waiting drains and
discards stdout and stderr concurrently without accumulating either stream.
Cancellation drops and kills/reaps the child. Normal wait also reaps it. These
use the existing `testrun` scope recipe and core's draining wait. The runner is
sequential; an Atomic counter supplies explicit state to the scoped retry closure.

Only listed exit codes retry. Spawn errors, signals, deadline expiry, clock/output
errors and nonretryable exits stop immediately. Reports use fixed status strings
(`success`, `exit:N`, `signal:N`, `timeout`, `spawn_not_found`,
`process_not_granted`, `process_error`, `clock_error`, `output_error`,
`schedule_interrupted`). Exit statuses are 0 for success/help, 1 for command failure,
2 for typed `InvalidInput` reports, and 3 for absent runtime capabilities. A missing
input-directory grant is an invalid-input report. Output transport errors retain
the launcher's normal error behavior.

The focused Rust harness `retryrun_23` copies every source/fixture via
`include_str!`, builds LLVM and Cranelift executables, then compares stdout,
stderr, exit code and effects with independent literal expectations. Its private
Python child records each PID and fails twice then succeeds, always fails, exits
permanently, stalls, or fills both pipes. Every PID must be gone after execution;
input hashes must remain unchanged and private state is removed. A separate shipped-workflow test checks the checked-in grants template.
The policy test uses virtual monotonic time on H and checked-H for policy boundaries and pure
model checks. Runtime processes have 60 CPU-s/2 GiB limits; real retry sleeps for
each fixture total under 100 ms. No socket is used.

Reference vectors are ported from Cenk Altı's Go backoff
`6f228bfdb51209aa375869feb27dcede4fa0db06`, `retry.go:65–97` and
`exponential_test.go:9–38`, and Sam Rijs's tokio-retry
`69a8766aaead1190c2a305937d45740d6c2d9b33`, `src/strategy.rs:235–264`.
Their MIT licenses are preserved in [LICENSES.txt](LICENSES.txt). The Go delay
vector is scaled from milliseconds to nanoseconds and its symmetric jitter is
omitted. Hirsel's strict elapsed equality is retained; the optional zero attempt
limit from the general scheduler is rejected by this application's policy.
Tokio's Fibonacci vectors exercise the shipped scheduler only; the application's
policy remains exponential. No scheduler implementation was ported.
