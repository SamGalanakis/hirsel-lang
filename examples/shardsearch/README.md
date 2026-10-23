# shardsearch

Search an explicit ordered file list for the first K non-overlapping literal
matches. Run from this directory with a source-built `hirsel`:

```sh
hirsel run hirsel.json --grants grants.json
# 0:3
# 0:7
# 1:8
hirsel run --native --backend llvm hirsel.json --grants grants.json --cores 2
hirsel run hirsel.json --grants grants.json -- fixtures foo 2 1 second.txt first.txt
# 0:8
```

Arguments are `ROOT QUERY JOBS K [FILE ...]`. ROOT is a virtual read-only directory
grant; the template maps `fixtures` to this directory's fixtures. Grant host paths
are relative to the grants file; instantiate an absolute host path for external
fixtures. File operands are relative to ROOT, duplicates are separate input slots,
and there is no directory enumeration. Paths retain the provider's confinement
checks. No regex, case folding or normalization is performed.

Stdout contains `FILE_INDEX:BYTE_OFFSET\n` per match, both zero-based. UTF8 text
`é foo` places `foo` at byte 3, not character 2. Query and file contents must be
valid UTF8; invalid file bytes fail as `contents_utf8:INDEX`. Files are validated
in full before searching. Errors print `KIND:FILE_INDEX\n` to stderr, with exit 2
and empty stdout. Invalid global arguments use index 0. Success, including no
matches or no files, exits 0. Output failures propagate; missing output grants
exit 3.

Bounds: 0..64 files, nonempty query at most 256 bytes, jobs 1..8, K 1..64,
64 KiB per file and 1 MiB aggregate payload reads. As in
`file.read_all_bounded`, each file permits one extra byte of lookahead to reject
excess input (at most 64 extra probe bytes across the run). A full file is read in
4096-byte requests before searching, so a query crossing a read boundary works.
Matching uses existing `string.find` byte search on valid UTF8 views; each pure
search is bounded by 64 KiB. Cancellation is checked between matches and by file
reads. Pure native search is not asynchronously interrupted.

At most JOBS children run in one batch, with at most K offsets buffered per file.
The query uses one immutable Shared root; each worker owns its path copy. Completed
results stay in TaskSet slots. Only consecutive input slots are committed. Once
that prefix proves K matches, `task.cancel_all` cancels and joins every remaining
child before any output is printed. A completed speculative error beyond the
cutoff is discarded. A file error within the required prefix fails the search.
Each batch reserves its maximum payload reads against the remaining aggregate
budget; near 1 MiB it may run fewer than JOBS children. Unused reservations are
returned after a complete batch. This trades pipeline refill for simple bounded
memory and deterministic resource accounting.

The permanent CLI tests execute this application, rather than a replacement
model, with the public reference and checked-H adapters and both native backends.
Each successful vector is checked against an independent sequential byte oracle
at logical jobs 1/2/4. Provider waiting and physical placement/replay qualification
have separate receipts in the external lane report; ordinary engine agreement
does not establish physical worker width.

Prior art: Tokio `ce685ee6916549ed7e52dbfd5430858bbdc734d3`,
`tokio/src/task/join_set.rs:381–384` and `tokio/tests/task_join_set.rs:267–294`
(MIT), supplies the cancel-then-drain structure. Hirsel's `cancel_all` combines
those operations and this application awaits input order rather than Tokio
JoinSet completion order. Go `ebe2feefc400f813aefdc8a036e66d2619da6649`,
`src/bytes/bytes.go:1318–1356` and `src/bytes/bytes_test.go:143–184`
(BSD-3-Clause), supplies literal byte-index expectations. The 38 nonempty
short-string vectors are adapted to repeated non-overlapping matches, stopping at
K; empty query is rejected before std search. Existing std substring search is
used without porting Go SIMD/bytealg machinery. Non-UTF8 subjects are rejected
as explicit application policy. License texts are in
[third-party notices](../../third-party/NOTICES.md).
