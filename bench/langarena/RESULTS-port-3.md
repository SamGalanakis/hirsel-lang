# Compress and Sort port validation

Source: LangArena `3c9f84ddaeec834c0631747cb789e8cfd602f993`,
`rust/src/benchmarks/compress.rs`, `sort_benchmark.rs`, `sort_quick.rs`,
`sort_merge.rs`, `sort_self.rs`, and `rust/src/helper.rs`.
All eleven tasks pass both the unchanged `test.js` and `run.js` checksums,
using native LLVM release builds. No tasks are blocked; no compiler or standard
library changes are needed.

The scoped Buck2 target `//:gate-langarena` passes its ten protocol/comparison
tests and exercises every registered quick checksum (coverage grew from seven
tasks to eighteen). Two new native tests exercise sorting boundaries, BWT and
LZW byte vectors, exact Huffman/arithmetic bytes from the unchanged Rust
functions, and codec roundtrips at sizes 0, 1, 2, 11, 53 and 257. Binary bytes
are also exercised where upstream supports them; upstream Huffman deliberately
omits zero-valued leaves when decoding.

## Quick comparison

Reproduce from the repository root:

```sh
python3 bench/langarena/compare.py --quick --filter Sort --filter Compress \
  --session compress-sort-quick --output /tmp/compress-sort-quick.md
```

The session uses ten seeded interleaved rounds on CPU31 under the harness timing
lock, with cycles and instructions collected by perf. The runtime interval
excludes warmup. Instruction counters cover the whole process, including
preparation and warmup. The unchanged Rust runner prints only three decimals
(`rust/src/main.rs:393`); a zero-duration median cannot yield a runtime ratio.
Quick workloads do not support projections onto published full-size results.

Session `la-port-3-20261021` completed with all 220 checksum checks passing
(11 tasks × 2 languages × 10 rounds). Every task below also passes `run.js`.

| Task | Status | Quick runtime H/R | Instructions H/R |
| --- | --- | ---: | ---: |
| Sort::Quick | pass | 1.208 | 1.468 |
| Sort::Merge | pass | 1.085 | 1.796 |
| Sort::Self | pass | 7.370 | 6.253 |
| Compress::BWTEncode | pass | unavailable | 2.499 |
| Compress::BWTDecode | pass | unavailable | 2.517 |
| Compress::HuffEncode | pass | unavailable | 2.528 |
| Compress::HuffDecode | pass | unavailable | 2.549 |
| Compress::ArithEncode | pass | unavailable | 2.509 |
| Compress::ArithDecode | pass | unavailable | 2.517 |
| Compress::LZWEncode | pass | unavailable | 2.121 |
| Compress::LZWDecode | pass | unavailable | 2.139 |

All eight compression Rust medians print `0.000s` (below 0.5 ms); their
runtime ratios are unavailable. Counters include substantial startup cost for
these short tasks. Sort::Self uses Hirsel's WikiSort versus Rust's standard
stable sort, so its ratio also reflects different standard sort algorithms.

Consumed Hirsel source SHA-256:
`af85ec9a5c74a37c6d4e221625c22c30b9abd23d1a2687cda7af361cc3bf1538`.
Hirsel executable SHA-256:
`ca448f21a84b54f0e92d43fa844542df78a1f979dd7222394d4f0dc602420ba7`.
Rust executable SHA-256:
`34f46c760fcc739816add285f1328cd3fb8eef465143303f6c96ef7349dc8cb3`.
The comparison driver checks source, toolchain and executable identities across
the session and retains raw samples, counters, logs and commands in its ignored
`build/la-port-3-20261021/measurements.json`.

## Representation adaptations

- Numeric lists replace Rust arrays and vectors, with u64 indices and frequency
  counters. Sort retains the original i32 element width and floating PRNG
  conversion, resetting only the PRNG after warmup.
- Quick and Merge pass index ranges in place of mutable slices; copy and merge
  loops retain upstream preparation and per-iteration allocation boundaries.
  Self calls Hirsel's stable standard sort, just as Rust calls its stable sort.
- BWT sorts records containing rank pairs and suffix indices. A previous-order
  tie breaker preserves Rust's stable ordering without a capturing comparator.
- Huffman retains boxed tree nodes and frequency-ordered insertion. Initial
  leaves use stable insertion ordering. The bit-count-sized decode allocation
  is retained; repeated list.pop replaces the unavailable list.truncate.
- Arithmetic coding uses the same u64 interval arithmetic and MSB-first bit
  streams, with explicit binary partition-point lookup.
- LZW uses immutable Bytes map keys in place of Vec<u8> keys, copying the current
  byte list for lookup. Decoder entries remain byte lists; explicit copy and
  append loops replace clone and extend_from_slice.

These are collection/API adaptations; the benchmark algorithms and state
retention remain those of the upstream implementation.
