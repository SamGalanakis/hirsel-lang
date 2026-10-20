# Tasks runtime/std microbenchmarks

Build each `.hir` with `hirsel build --release ... -o <binary>`, with the
matching toolchain's runtime archives, LLVM and link kit (as in
`bench/micro/build`). The sidecar selects the std source. Build the baseline
against std from the baseline revision; build the candidate against current std.
Use `json_record_dom.hir` for the baseline and `json_record.hir` for the direct
writer. Both print the same wrapping checksum. The other programs use identical
source except `search`, whose baseline expression is
`string.contains(string.casefold(s), string.casefold("benchmark"))`.

Each binary takes N and epochs after
`--hirsel-grants bench/micro/grants.json`. Suggested N is 20,000, epochs 1.
The decimal workload covers the short-integer path repeatedly. Unicode folding,
search, normalization and JSON parsing include real call sites, not substitute
implementations. `json_record` compares a derived DOM/map with a derived writer.

`crates/hirsel-rt/examples/failure_table.rs` calls the real runtime loader without
requesting diagnostic text. Build `//:hirsel-rt-example-failure_table` on both
revisions with `--target-platforms //tools/buck2:release`; arguments are site
count and rounds (10,000 and 100). It prints those inputs as its checksum.

Put old and new commands in a participants JSON as described in
[the micro harness](../micro/README.md), then run:

```sh
bench/micro/run participants.json --reference before --seed 20261020 --output session.json
```

For the five application rows, `tasks-compare` reuses the tasks comparison
lane's server, seeded 10k board, open-loop load generator and counter protocol:

```sh
bench/tasks-perf/tasks-compare --harness bench/apps/tasks \
  --before /tmp/tasks-before --after /tmp/tasks-after --output /tmp/tasks-session
```

It runs ten seeded interleaved rounds of old Hirsel, new Hirsel and the locked
Rust current-thread executable. Servers use CPU31, clients CPU7. The CPU31
measurement lock covers the session, and raw samples retain CPU15 busy share,
all hardware counters and binary hashes. Every cell has a fresh server. Search,
create and update use 1 req/s for one second; cold uses an empty board and
restart uses 10k tasks. The instruction ratio is the verdict; cross-session
wall times are never compared. Missing samples fail the run.
