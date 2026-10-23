# rpcgate

A finite Unix RPC gateway with explicit routes, per-request deadlines and
bounded retries. Start the two fixture peers in separate terminals:

```sh
python3 examples/rpcgate/fixtures/peer.py /tmp/rpcgate-arith.sock
python3 examples/rpcgate/fixtures/peer.py /tmp/rpcgate-echo.sock
```

From the repository root, instantiate private grants and build a backend:

```sh
export HIRSEL=/absolute/path/to/hirsel
"$HIRSEL" run examples/rpcgate/hirsel.json --grants examples/rpcgate/grants-check.json
"$HIRSEL" build examples/rpcgate/hirsel.json --backend llvm -o /tmp/rpcgate-llvm
/tmp/rpcgate-llvm --hirsel-record /tmp/rpcgate.tape --hirsel-grants examples/rpcgate/grants-serve.json serve routes.json /tmp/rpcgate.sock
```

The grants templates mount `fixtures` relative to their own directory, under
virtual `config`. The serve template grants exactly the three named Unix paths.
To change them, edit both the private route manifest and private grants. Keep
recordings outside granted directories. Standalone observer/record flags precede
`--hirsel-grants`; application arguments immediately follow its operand. The
CLI's `run --grants` uses the template's Args unless explicitly overridden.

Send one request per connection from another terminal (the fixture manifest
admits two connections, then exits after they disconnect):

```sh
python3 examples/rpcgate/fixtures/client.py /tmp/rpcgate.sock '{"id":"a","method":"Arith.Add","args":{"a":7,"b":8}}'
python3 examples/rpcgate/fixtures/client.py /tmp/rpcgate.sock '{"id":"a","method":"echo","args":"hello"}'
/tmp/rpcgate-llvm --hirsel-replay /tmp/rpcgate.tape
```

`check FILE` validates a basename in the read-only config mount before projecting
network or task authority. `serve FILE SOCKET` reads that manifest, binds the
socket and emits `{"listening":true}`. At shutdown it joins every connection,
request and upstream attempt, then writes ordered attempt rows and a final
`clients`/`expired` row. Exit0 means check or a clean finite serve;1 means session
expiry;2 invalid input/config;3 a missing grant or I/O failure. Request/domain
errors are framed responses, not process or language failures.

The manifest has exactly `jobs`, `clients`, `request_ms`, `session_ms`, `routes`.
Each route has exactly `method`, `socket`, `idempotent`. Methods match byte for
byte. Sockets are canonical absolute paths of2..100 UTF-8 bytes. Bounds are4096
manifest bytes,8 routes,8 total clients,16 admitted requests including queued
replies,1..4 active upstream attempts,64 requests per connection,3 attempts per
request,1..3000ms per request and1..15000ms per serve session. A full request
queue returns `busy` before spawning another child. Admitted requests waiting for
an upstream slot keep their request credit and share their absolute deadline.

Frames are localrpc's4-byte big-endian length followed by1..2048 UTF-8 JSON bytes.
Requests contain exactly `id`, `method`, `args`; IDs are1..64 bytes and unique
within a connection. `args` is either `{a:i64,b:i64}` or a string up to512 bytes.
Replies contain `id` plus either a typed `result` (i64 or bounded string) or
`error:{code:String}`. The gateway validates shape and correlation, passes domain
errors through without retrying, and replaces the upstream ID with the original
client ID. A client may pipeline requests; replies may arrive out of order.
Leave the client's send direction open while waiting: EOF, including half-close,
means disconnect and cancels all associated work.

Every attempt owns a fresh upstream socket and ID `c<client>r<request>a<attempt>`.
Only an explicitly idempotent route retries transport refusal/reset/abort,
broken pipe, timeout or clean EOF before a reply. Truncated/malformed frames,
wrong/missing IDs, grant errors and application errors never retry. The shipped
schedule policy supplies10ms then20ms backoff, including the first operation in
the3-attempt cap. One absolute deadline starts at admission and covers scheduling,
capacity waiting, connecting, writing, reading and backoff. Non-idempotent
transport loss after connect reports `unknown_outcome`, conservatively including
failed writes; no exactly-once delivery claim is made. An unsafe route whose
request deadline expires also reports `unknown_outcome` conservatively.

Module responsibilities are explicit in hirsel.json: main owns CLI/capability
entry and finite serve; config owns immutable validation; wire owns framing and
generic envelopes/codecs; route owns downstream reader/writer/request trees;
attempt owns upstream sockets, retry policy and the deadline body; report owns
credits, bounded records, canonical row order and joined shutdown. The typed
seams execute with Numbers/i64 and String/String; no JSON payloads enter reports.

The focused Rust harness is `rpc_gateway_30` in CLI shard1. Small app workflows
use4M reference steps; requests64/65 declare12M before first execution. Step
usage is retained separately; the global default stays1M. See FRICTION.md for
original failures, coverage limits and per-revision receipts. Production Ordered
qualification uses normal grants at widths1/2/4 and fixed jobs4, pausing on the
listening handshake before sampling running/effectiveN/pending0. Those receipts
prove capacity, not simultaneous hardware use or performance.

Prior art: Go `net/rpc/client.go:71–169` and `server_test.go:165–301`, commit
`ebe2feefc400f813aefdc8a036e66d2619da6649`, supply call correlation, terminal
cleanup and arithmetic/error/out-of-order vectors. Tokio
`tokio/src/sync/mpsc/bounded.rs:760–830` and `tests/sync_mpsc.rs:48–130`, commit
`ce685ee6916549ed7e52dbfd5430858bbdc734d3`, supply reserve-before-send and
return-capacity-on-drop structure. LICENSE-Go and LICENSE-Tokio preserve their
licenses. Deviations: JSON/Unix replaces gob/reflection/HTTP; structured scopes
replace goroutines; explicit capabilities replace ambient effects; bounded
application credits/timers replace Tokio's semaphore; fresh per-attempt sockets
replace Go's shared connection; deadlines and disconnect cancellation are added.
Framing and JSON implementations are reused from localrpc/std, not reimplemented.
