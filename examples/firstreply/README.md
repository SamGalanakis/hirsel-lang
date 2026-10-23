# firstreply

Query equivalent read-only localrpc responders and report the **first completion**,
including an error. A fast refusal or malformed frame wins over a later success.
There are no retries. Use the endpoint index to identify the chosen socket in the
argument list. Timeout has no chosen endpoint. The client emits no elapsed time.

```sh
hirsel run examples/firstreply/hirsel.json --grants examples/firstreply/grants.json
hirsel build --backend llvm examples/firstreply/hirsel.json -o /tmp/firstreply
/tmp/firstreply --hirsel-grants /absolute/path/to/grants.json 1000 \
  '{"id":"a","method":"normalize","text":"  HELLO  "}' \
  /tmp/service-a.sock /tmp/service-b.sock
```

The shipped grants print help through `hirsel run`. Standalone executables take
the application arguments immediately after the grants path; no `--` separator
is used in that form. For an actual query, instantiate this template
with your absolute canonical socket paths (the sockets must already be listening):

```json
{
  "args": ["1000", "{\"id\":\"a\",\"method\":\"normalize\",\"text\":\"  HELLO  \"}", "/tmp/service-a.sock", "/tmp/service-b.sock"],
  "io": {
    "stdout": "stdout.bytes@1",
    "tasks": "tasks.cooperative@1",
    "monotonic_clock": "clock.monotonic.ns@1",
    "dirs": [],
    "net": {
      "provider": "net.stream@1",
      "rules": [
        {"address":"/tmp/service-a.sock","service":"0","operation":"unix_connect"},
        {"address":"/tmp/service-b.sock","service":"0","operation":"unix_connect"}
      ]
    }
  },
  "placement": {"cores":2,"commit":"shared","reorder":"auto"}
}
```

Start responders with [localrpc](../localrpc/README.md), or another compatible
peer. Requests are the existing strict Ping/Normalize JSON shapes, framed with a
four-byte big-endian length. Only the first reply frame is consumed and validated
against the request ID and expected result. Unknown methods are rejected locally.
Bodies are 1..2048 bytes; IDs 1..64 bytes; Normalize text at most 512 bytes. Supply
1..4 endpoints and a timeout in milliseconds, 1..60000. Empty sets, excessive
endpoints, bad deadlines and invalid local requests are rejected before network
or task authority is projected. Duplicate endpoints deliberately mean separate
requests. The one absolute deadline covers connecting, writing, and reading across
all children; an error does not start a fresh deadline.

Success emits exactly one JSON line, for example
`{"endpoint":1,"response":{"id":"a","result":"hello"}}`.
A selected error emits `{"endpoint":0,"error":"connection_refused"}`;
expiry emits `{"error":"timeout"}`. Exit statuses are 0 for success, 1 for a
selected error/expiry, 2 for invalid local input, and 3 for grant/clock/output
failure. Peer error codes include localrpc framing/validation codes,
`connection_refused`, `connection_reset`, `not_granted`, `network_timeout`,
`canceled`, and a `network` fallback.

Each child owns independent request bytes, a parsed request and its result. Its
stream retains its originating Net brand. `task.select` consumes the set, cancels
and joins all losing children, and only then returns. An enclosing deadline scope
also joins before returning. No handle escapes; all streams close before output.
Cancellation cannot retract requests already delivered to a server. This tool is
therefore limited to equivalent read-only methods.

The focused `firstreply_27` CLI tests run the real orchestration using scripted
providers, plus independent Unix peers with readiness handshakes. Ten named
workflows and their exact stdout/status/cleanup expectations live in
[fixtures/workflows.json](fixtures/workflows.json). Checked H uses the public
`run_observed_checked` adapter; native execution uses the shipped launcher. No
pure model is substituted for a missing engine cell. External campaign receipts
and unresolved qualification cells are maintained in the lane report.

Prior art: Tokio `JoinSet::shutdown` (`tokio/src/task/join_set.rs:381–384`),
`task_join_set.rs:267–295`, and `macros_select.rs:93–146`, commit
`ce685ee6916549ed7e52dbfd5430858bbdc734d3`, MIT, Copyright Tokio contributors.
The algorithm/vector shapes are adapted through the existing Hirsel std library:
bounded 1..4 children instead of ten; explicit owned results instead of futures;
`task.cancel_all` performs abort+drain; completed errors are eligible winners;
FIFO completion order replaces Tokio's randomized tie selection. No Tokio
executable is the oracle for Hirsel ties. Framing reuses localrpc unchanged; see
its Go/Tokio provenance and license notes. The independent Python oracle uses
`struct.pack('!I')` and JSON, never the Hirsel encoder to derive expected output.

Qualification records observed native effective widths 1/2/4 for success,
deadline and malformed-reply workflows on both backends. Their 18 immutable
recordings replayed in 126 interpreter/native cells with peers removed. The
separate pending-connect replay and canonical-trace issues remain open; see
[FRICTION.md](FRICTION.md). Recorded LLVM wall-time observations are UNCERTIFIED
and do not establish a speedup or effective placement for the short timed runs.
