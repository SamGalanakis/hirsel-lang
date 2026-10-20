# Network cost floor

`net-only.hir` accepts a single connection, reads 1024 fixed 35-byte requests
with `net.read_into`, and writes constant responses. It excludes HTTP parsing,
connection transfer, and application work. Keep the source frozen when comparing
runtime changes. The buffer initializer intentionally remains inside the loop:
its Hirsel semantics preserve the unwritten suffix.

Build with `hirsel build bench/micro/net/hirsel.json --release -o /tmp/net-after`.
Build the independent reference in `../http/server.rs` with `rustc -O`.
Run `compare --rust /tmp/rust --binary before=/tmp/net-before
--binary buffers=/tmp/net-buffers --binary after=/tmp/net-after --record --output /tmp/net-counts`.
The wrapper uses the existing CPU31 perf harness and CPU7 open-loop client,
10 seeded interleaved rounds, and requires identical output checksums. The Rust
reference recognizes the fixed HTTP request; Hirsel counts its fixed length.
Both send 1024 equal-sized application bodies. User instructions are the verdict;
retain total instructions and cycles to expose kernel work as well.

`--record` adds a participant using the native `--hirsel-record` flag for each
binary. Plain mode is the performance target; report recorded mode separately.
