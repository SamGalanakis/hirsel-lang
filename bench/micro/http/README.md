# HTTP keep-alive micro

Both finite servers use one thread on CPU31, accept one persistent connection,
parse 1024 GET requests and return `Hello, world`. The Hirsel server uses std's
HTTP inline adapter (`serve_next`); the Rust reference is a minimal std TCP parser, so its
narrower grammar and validation are part of the comparison. This is not an
application, idle-connection scaling or saturated-throughput benchmark.

Build `server.hir` with the release CLI and `server.rs` with `rustc -O`, each
under `tools/capped`. Grant Hirsel stdout, `tasks.cooperative@1`,
`clock.monotonic.ns@1` and `net.stream@1` listen on 127.0.0.1:18475. Rust listens
on 18476. Both print `1024` after their final response.

Start `load.py --output latency.json` through a separate capped invocation,
pinned to CPU7, then use `bench/micro/run` on the server participant list.
The client offers 2048 requests/second regardless of responses and records
latency from the scheduled send time, including queueing. Hardware counters
cover only each fresh server process; the client is outside its process tree.
The harness holds the CPU31 session lock and records CPU15 activity. The keep-alive client
makes one connection per server process. Its port identifies the participant. Do not run two such sessions concurrently.

The Rust accept/read/write structure follows Rust std's
`library/std/src/net/tcp.rs:904-915` (`TcpListener::accept` example). The sole
deliberate deviation is recognizing the fixed fixture request and returning a
fixed response, so this comparator measures a TCP cost floor rather than
implementing the general HTTP protocol.

`cargo build --release --locked --manifest-path bench/micro/http/tokio/Cargo.toml`
builds the Tokio reference (Cargo may place it in the managed target directory).

`tokio/` is a general httparse request parser over a single-thread Tokio runtime,
with no per-request task. It uses the same finite 1024-request workload and prints
`1024`. Its default mode keeps a connection alive on 18476; `hello` accepts a new
connection per request on 18478. The Hirsel hello fixture is the production
`examples/hello-web/main.hir` bounded to 1024 accepts, using port 18477 and printing
`1024` at exit. This finite form counts startup and all request work and lets the
existing micro harness check output equality.

`compare --participants participants.json --output measurements.json` uses the
shared timing lock, ten seeded interleaved rounds, CPU31 servers, CPU7 open-loop
clients and the micro harness's cycles/instructions/user counters. Each participant
adds `port` to the ordinary micro participant record. Name the Tokio references
`rust-hello` and `rust-keep`, and append `-hello`/`-keep` to Hirsel variant names.
The client rate is 2000 new connections/s for hello and 2048 requests/s for
keep-alive. The session JSON retains every sample and the per-round client files
record latency. Ratios are computed within the session, separately per workload.

Generate that finite fixture with
`python3 bench/micro/http/hello_fixture.py /tmp/http-hello/main.hir`, then build
it with the release CLI and normal runtime/link environment. `--source` and
`--stdlib` select a saved source/library version for a controlled comparison.
Grant listen on 127.0.0.1:18477, stdout and cooperative tasks, matching the
original example's authority. Keep-alive also needs the monotonic clock grant.

See [RESULTS.md](RESULTS.md) for the controlled before/after session, intermediate
changes, prior-art deviations, validation limits and the remaining performance gap.
