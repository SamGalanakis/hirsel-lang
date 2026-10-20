# HTTP keep-alive micro

Both finite servers use one thread on CPU31, accept one persistent connection,
parse 1024 GET requests and return `Hello, world`. The Hirsel server uses std's
HTTP stream adapter; the Rust reference is a minimal std TCP parser, so its
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
The harness holds the CPU31 session lock and records CPU15 activity. The client
makes twenty connections, one per participant per round. Its port identifies
the participant. Do not run two such sessions concurrently.
