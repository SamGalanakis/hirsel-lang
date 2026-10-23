# Unix pubsub relay

Run the interpreter from the repository root with the shipped grants:

```sh
hirsel run examples/pubsub/hirsel.json --grants examples/pubsub/grants-serve.json -- serve /tmp/pubsub.sock 64 10000 4
```

After its listening handshake, the client commands in another terminal are:

```sh
hirsel run examples/pubsub/hirsel.json --grants examples/pubsub/grants-client.json -- subscribe /tmp/pubsub.sock news 1
hirsel run examples/pubsub/hirsel.json --grants examples/pubsub/grants-client.json -- publish /tmp/pubsub.sock news hello
hirsel run examples/pubsub/hirsel.json --grants examples/pubsub/grants-client.json -- stop /tmp/pubsub.sock
```

Run subscribe and publish concurrently; connections last at most two seconds.
The automated Python peer below gives a reproducible, handshake-driven example:

```sh
python3 examples/pubsub/fixtures/peer.py hirsel
```

Copy the templates and replace their exact socket address to run in a private
directory. The grants authorize only that pathname; no ambient Net authority is
used. Exit 2 from an invalid command is a domain refusal, not a language failure.

Build and run the native app with either LLVM or Cranelift:

```sh
hirsel build examples/pubsub/hirsel.json --backend llvm -o /tmp/pubsub
/tmp/pubsub --hirsel-grants examples/pubsub/grants-serve.json serve /tmp/pubsub.sock 64 10000 4
```

Record/replay options precede `--hirsel-grants`; application arguments follow its
operand. The native qualification test covers live Unix workflows, requested
widths 1/2/4, and immutable replay; see FRICTION.md for remaining obligations.

The server prints `{"listening":true}` after bind. Wait for that handshake before
connecting. `serve SOCKET ADMISSIONS MILLISECONDS JOBS` stops when the admission
limit drains, on a stop command, or at its global deadline. Jobs (1..4) bounds
staged admissions; runtime worker placement is an independent grants member.
There are eight lifetime connection IDs, eight retained topic names across connection records, four outstanding admissions, four credited deliveries per subscriber,
64 admissions and 2048 bytes per JSON frame. A connection lasts at most two
seconds. Admission reservation waits and socket writes are bounded by two
seconds. Sessions last at most ten seconds. There is no reconnect/resume state.

Frames use localrpc's unsigned four-byte big-endian payload length followed by
UTF-8 JSON. A length is checked before reading or allocating a body. Empty,
truncated, oversized, invalid UTF-8/JSON and unknown/extra fields close that
connection with a protocol reason. Topic strings match exactly, contain 1..64
UTF-8 bytes and are never normalized. Publish data is at most 192 UTF-8 bytes;
this leaves room for worst-case JSON escaping in the bounded delivery frame.

Commands:

* `{"op":"subscribe","topic":"news"}` replies `subscribed`. Duplicate subscribe
  is idempotent and does not add a second delivery.
* `{"op":"unsubscribe","topic":"news"}` replies `unsubscribed`, also for a miss.
* `{"op":"publish","topic":"news","data":"hello"}` replies `queued` with `seq`
  and `subscribers` after all captured subscriber credits have been reserved.
* `{"op":"stop"}` replies `stopping`; the server drains credited output then joins.

Deliveries contain `op:delivery`, `seq`, `topic` and `data`. The central loop
commits publishes in FIFO order, assigning increasing sequence numbers only at
commit. Subscriber IDs are reserved in increasing order. Each staged publish
captures its recipient set. Membership changes or disconnects affecting that
snapshot refuse the pending publish and return all partial reservations. A
subscriber joining later receives only later admitted publishes. A reservation
timeout disconnects the blocking subscriber with `slow_reader`; no message is
silently overwritten. The central loop processes control requests and checks
pending admissions without blocking on credit acquisition.

A command-credit envelope bounds each reader's mailbox contribution. A separate
control slot bounds replies. Delivery-credit envelopes survive channel dequeue
and return their credits on successful write, failed send/write, EOF, cancellation
and buffered-message destruction. Read and write halves have separate task
owners. The supervisor cancels and awaits both; the accept scope joins every
connection before the final report. `queued = written + disconnected` and
`reserved = returned` with `credits_held = 0` are checked at the final boundary.
A successful socket write proves local provider acceptance, not remote
application consumption; there is no exactly-once or durability claim.

Exit 0 covers a completed finite session; exit 2 is invalid CLI input or a
resource-limit refusal; exit 3 is capability or I/O failure. Individual peer
protocol errors and disconnects are reported while the server continues.

The six modules are explicit in hirsel.json: main (CLI/capability entry), wire
(codec/schema), hub (subscriptions/admission), client (split ownership/lifecycle),
flow (generic envelopes/credit destruction), shutdown (deadlines/reasons/report).
The generic envelope is instantiated with command and delivery/control-message
payloads. Frozen payload roots make cross-task sharing explicit; credit ownership
is always unique and never copied.

Prior art: Tokio commit ce685ee6916549ed7e52dbfd5430858bbdc734d3,
`tokio/src/sync/mpsc/bounded.rs:760-830`, `tokio/tests/sync_mpsc.rs:48-130`,
`tokio/src/sync/broadcast.rs:1-68`, `tokio/tests/sync_broadcast.rs:98-150`.
MIT license is preserved in LICENSE-TOKIO. The framing state machine is reused
from examples/localrpc/model.hir (tokio-util length_delimited.rs:504-568 at the
same revision). Deviations: no broadcast overwrite/lag policy; explicit permit
reservation and disconnect instead. Hirsel scope joins replace Tokio spawn/drop
lifetimes. Credits last through socket writes, beyond channel dequeue. Bounded
lists replace the general semaphore/linked waiter structures at these tiny caps.
No new std or runtime adapter is introduced.

Verification and known limitations are recorded in FRICTION.md. Evidence, original
failures, hashes, receipts and tapes live outside the repository under
`/workspace/code/hirsel-briefs/pubsub-relay-30/`.

The six application modules total 1245 handwritten lines: main 99, wire 212,
hub 445, client 249, flow 167, shutdown 73. Fixtures, tests, licenses and
documentation are excluded.

Focused verification uses `pubsub_relay_30` on the CLI shard-1 target. Four tests
cover complete scripted workflows, boundaries, actual credit finalizers, real
Unix peers and reference/checked-H replay. The fifth exercises LLVM and Cranelift
builds, scripted workflows, live Unix peers and immutable tape replay. Slow-reader
and partial-reservation obligations remain separate; the independent event model
is supplementary evidence only.
