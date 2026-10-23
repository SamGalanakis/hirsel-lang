# batchrun

Run finite local validation commands concurrently and report their statuses in
manifest order. Child stdout and stderr are **drained and discarded**; this tool
never captures output. No shell, retries, remote commands or process-tree policy.

From the repository root, using a source-built CLI with its matching std image
and native toolchain (see [native verification](../../docs/native-verification.md)):

```sh
"$HIRSEL" run --native --backend llvm examples/batchrun/hirsel.json \
  --grants examples/batchrun/grants.json --cores 2
```

The checked-in example runs `/usr/bin/true` twice. Its grant allows only that
exact executable, and resolves `host: "fixtures"` beside the grants file. To use
your own manifest, copy the template, give `io.dirs[0].host` an absolute directory
containing your JSON, set `args` to its filename, and explicitly grant each
canonical executable under `io.process.rules`. Arguments are literal UTF-8
strings; metacharacters have no shell meaning. Processes receive no environment
variables or working-directory capability. They start in `/` with stdin closed.

```json
{"jobs":2,"timeout_ms":10000,"commands":[
  {"argv":["/usr/bin/true"],"timeout_ms":5000},
  {"argv":["/usr/bin/false"],"timeout_ms":5000}
]}
```

The manifest must have exactly these fields. Bounds are 256 KiB input, 0..16
commands, `jobs` 1..4, 1..64 words per argv, and 8192 UTF-8 bytes per argv including
the program and one terminating NUL per word. Embedded NULs, relative or
noncanonical programs, extra fields and zero/out-of-range deadlines are rejected
before any spawn. Child deadlines are 1..5000 ms and the run deadline is
1..30000 ms. A deadline bounds execution; cleanup and reaping finish before
return. An empty batch succeeds, but still requires the runtime grants.

At most `jobs` direct children are active. Each completion frees a slot immediately;
a failed command does not stop siblings. A whole-run deadline stops admission,
cancels and joins active jobs, and labels unfinished/unadmitted entries
`global_timeout`. Deadline cancellation can win before a newly spawned executable
runs any user code. Completed statuses remain available. Only direct children are
managed (`Command.own_group: false`); commands that create descendants are outside
this tool's cleanup guarantee.

One JSON object is printed per command, followed by a summary:

```json
{"index":0,"program":"/usr/bin/true","status":"exit","exit":0,"signal":null,"timeout":false}
{"index":1,"program":"/usr/bin/false","status":"exit","exit":1,"signal":null,"timeout":false}
{"commands":2,"expired":false,"output":"discarded"}
```

JSON key order is unspecified. Statuses also include `signal`, `timeout`,
`spawn_not_found`, `process_not_granted`, `process_error`, `clock_error`,
`canceled` and `global_timeout`. Exit status: 0 all commands succeeded/help,
1 a command failed or the run expired, 2 invalid input (a `kind` JSON error on
stderr), 3 a missing runtime projection. `--help` needs only stdout/stderr.

To build once and record/replay without compilation in the measured run:

```sh
"$HIRSEL" build examples/batchrun/hirsel.json --backend llvm -o /tmp/batchrun
/tmp/batchrun --hirsel-record /tmp/batchrun.tape \
  --hirsel-grants examples/batchrun/grants.json success.json
/tmp/batchrun --hirsel-replay /tmp/batchrun.tape
```

Keep tapes outside granted directories. A standalone recording option precedes
`--hirsel-grants`; a trace observer, when used, precedes both. `placement` in a
private grants file selects `cores`, `commit: "shared"`, `reorder: "auto"` and
private metrics/control paths. `HIRSEL_JOBS` controls compilation/tool caps; it
is not worker-placement evidence.

The complete app is tested with interpreter/checked-H scripted providers and
real interpreter/LLVM/Cranelift children, including both pipes (96 KiB each),
stdin EOF, bounded admission, deadlines and replay without the original files.
See [FRICTION](FRICTION.md) for the exact remaining boundary limitations; native
runtime execution, interpreter execution and physical placement are separate
observations. The accepted 8192-byte argv boundary exhausts the default 1M-step
interpreter budget and remains an incomplete semantic-engine boundary cell.

Prior art: Tokio `JoinSet::shutdown` and join-next aggregation at
`tokio/src/task/join_set.rs:374–395`, and abort/join tests at
`tokio/tests/task_join_set.rs:267–295`, commit
`ce685ee6916549ed7e52dbfd5430858bbdc734d3` (MIT).
Go `src/os/exec/exec.go:915–950` and `exec_test.go:470–525`, commit
`ebe2feefc400f813aefdc8a036e66d2619da6649` (BSD-3-Clause), supply the wait/join and
pipe-close sequence. This application ports those structures, with Hirsel's
structured scopes, owned commands, discard-only wait, domain failures, and
manifest-order slots instead of Tokio's completion-order vector. No upstream
executable or scheduler rule is imported.
