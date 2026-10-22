# LZW dictionary cost, 2026-10-22

Baseline: current main `d5b594d8250ad3c306e3caca6b68aa7cb5b754ba`; unchanged Rust anchor `3c9f84ddaeec834c0631747cb789e8cfd602f993`. LLVM 21.1.8 release; compiler, runtime archives, grants, runner and configurations fixed within each experiment. No language, IR, runtime ABI or dictionary representation change. Historical fidelity-8 numbers are not the baseline.

## Change and fidelity

Encoder lookup now uses `bytes.from_list(copy current)` instead of deep-copying `current`. The shared byte view dies on a hit before the next push. On a miss the dictionary retains that full byte sequence and `current` is replaced, matching Rust’s move into `HashMap<Vec<u8>, _>`. Every lookup still hashes and compares the complete key. Measured copy-on-write detachments remain zero.

The decoder is unchanged: 256 one-byte entries, reserve 4096, full old-entry clone plus one-byte append, bulk output extension, two-byte big-endian codes, and the next-code special case. No prefix-code table, integer-key substitution, capacity reuse across runs, or work moved outside the reference boundaries.

## Ten-round change experiment

Ten seeded interleaved rounds (seed `20261022`), three frozen participants, assigned cases only: 60/60 successful whole-process checksum runs. CPU 13, `nice -n 10`, `tools/capped`, `perf stat` task-clock/cycles/instructions plus user counters. Every counter has 100% running coverage. Counts include startup, preparation and warmup; this experiment claims instruction and allocation reductions, with no before/after wall-clock claim. CPU 29 is its SMT sibling; its activity snapshots are retained.

| Task | Rust instructions | Before instructions | Candidate instructions | Candidate / before | Before / Rust | Candidate / Rust |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Compress::LZWEncode | 9,370,731,841 | 20,177,362,875 | 12,899,009,773 | 0.6393 | 2.153 | 1.377 |
| Compress::LZWDecode | 9,228,642,466 | 11,728,351,182 | 11,351,078,346 | 0.9678 | 1.271 | 1.230 |

User-space instruction medians isolate kernel/page-fault variation:

| Task | Rust user instructions | Before | Candidate |
| --- | ---: | ---: | ---: |
| Compress::LZWEncode | 9,351,868,932 | 20,124,512,725 | 12,860,375,812 |
| Compress::LZWDecode | 9,201,680,307 | 5,624,922,408 | 5,245,038,930 |

Encode uses about 36.1% fewer instructions. The decode task uses about 3.2% fewer whole-process instructions because its preparation calls the encoder once; this is not decoder-kernel implementation credit. Its timed decoder body is unchanged.

## Allocation and construction evidence

Separate measurement-runtime builds; all allocations equal frees, zero live bytes at exit, zero COW detachments. Coverage is Hirsel runtime-managed heap allocations, including process setup/preparation/warmup; it excludes Rust/libc/compiler allocations and is not a cross-language allocation ratio.

| Task | Allocations before → candidate | Deep copies before → candidate | Copied physical bytes before → candidate |
| --- | ---: | ---: | ---: |
| Compress-LZWEncode | 21,232,155 → 831,645 | 20,400,510 → 0 | 2,373,665,664 → 0 |
| Compress-LZWDecode | 10,618,979 → 9,618,973 | 9,981,766 → 8,981,760 | 3,345,913,408 → 3,111,444,480 |

Encode removes exactly 200,005 transient deep copies per call × 102 warmup/timed calls = 20,400,510 copies; total allocations fall 96.1%. It retains all dictionary entries and the result. Decode removes exactly 1,000,006 preparation copies; its 4,678 clones per decode × 1,920 calls = 8,981,760 full entry copies remain. The measurement build’s source-site buckets are unlabelled (site 0), so totals are not presented as sampled per-site allocation attribution.

An additive diagnostic trace extracted the pinned Rust codec bodies and recorded capacities outside the unchanged reference participant. At encode size 200,006 the result is 4,174 bytes and the dictionary has 2,342 entries; size 1,000,007 produces 9,358 bytes and 4,934 entries. The Rust map’s reserve avoids all map growth in both cases. Hirsel has no shipped public map reserve operation, so its pre-existing map growth remains.

For a full decode call the outer dictionary grows once beyond its initial 4,096-entry reserve, the result grows six times from its 18,716-byte reserve to hold 1,000,007 initialized bytes, and 4,678 full entries are cloned (999,607 initialized source bytes) then appended. Rust Vec::clone allocates initialized length, so all 4,678 clone/appends reallocate in that diagnostic trace. Hirsel unique-copy preserves physical capacity, which differs from Rust and is included in copied-byte totals. Input generation and encoded preparation each occur once outside timing; output and dictionary construction remain inside every decoder call.

Fresh baseline `perf record -e cycles:u -g` profiles separately cover encode and decode. Encode samples: hash update 21.48%, map.contains 18.53%, memcmp 10.81%, allocation 10.79%, unique-copy 4.45%, clone_block 3.41%. Decode has unresolved/kernel buckets; allocation 9.16%, memmove 7.78%, decoder 6.26%, unique-copy 4.12%, reserve_byte_list 3.62%. These are diagnostic singleton samples, not speedup evidence.

## Locked candidate/reference confirmation

The requested `compare.py --upstream /tmp/ref-langarena --filter Compress::LZWEncode --filter Compress::LZWDecode --rounds 10 --session la-lzw-10 --output bench/langarena/RESULTS-la-lzw-10.md` also completed. The CPU31 lock was immediately available and held only for this session; no other holder was displaced. Seed `20261021`, 40/40 passing samples, 100% counter coverage; unchanged upstream was freshly cloned/pinned/built. Only ratios within this separate session are comparable. No ranking or before/after time speedup is inferred.

| Task | Candidate seconds | Rust seconds | Time H/R | Instructions H/R |
| --- | ---: | ---: | ---: | ---: |
| Compress::LZWEncode | 1.048885 | 0.826500 | 1.269 | 1.378 |
| Compress::LZWDecode | 2.340322 | 0.631500 | 3.706 | 1.230 |

## Correctness and reproduction

- Scoped Buck target `root//:hirsel-cli-test-la_lzw_10`: baseline 3/3 Rust tests and candidate 3/3, each enforcing 12/12 source tests on H, LLVM and Cranelift (36 before and 36 after). The standalone Cargo-discovered test file is automatically registered by Buck sync; this CLI harness has no `modules.rs` registry or `main.rs` registration to edit.
- Ten independent Rust-encoded fixtures compare every encoded byte and decode independently supplied Rust bytes: empty, zero/255 singleton, ABABABA, repeated byte, every byte, arbitrary binary, size60/61 patterns, deterministic 1,024-byte mixed input. Explicit ABABABA `[0,65,0,66,1,0,1,2]`, next-code `[0,65,1,0] → AAA`, and invalid `[0,65,1,1] → []` are retained. The existing adjacent BWT/Huffman/arithmetic source controls also execute.
- Full-size LLVM independent oracles compare all 4,174 and 9,358 encoded bytes from the pinned Rust encoder and complete decoded inputs of 200,006 and 1,000,007 bytes. Full-size performance retains the original checksum and full decoded-data equality boundaries.
- Quick correctness: before/candidate/Rust encode size60 checksum208 and decode size61 checksum100244, all six pass. Full-size counts retain encode425748 and decode1920113440. Odd encoded length still fails before dictionary allocation; an invalid first code still fails the initial dictionary index. These remain benchmark-internal malformed-input limits, not a general codec API.
- Hirsel formatting, Rust formatting and `git diff --check` pass. Landing performs build/Clippy/fmt on each rebased tree.

Rerun scoped correctness after `python3 tools/buck2/sync.py --if-stale`: `.buck2/bin/buck2 test root//:hirsel-cli-test-la_lzw_10`. The target actually executes three tests; no empty selection is accepted.

All raw samples, exact runner argv, grants, source/config identities, profiles, construction trace, immutable before/candidate/Rust binaries, compiler, toolchain and runtime are retained at `/workspace/code/hirsel-briefs/la-lzw-10/`. Reproduction scripts include `la-lzw-10-interleaved.py`, `la-lzw-10-alloc-build.py`, `la-lzw-10-alloc-run.py`, and `la-lzw-10-full-oracle.py`. `interleaved-counts.json` verifies consumed hashes and binaries unchanged after all 60 runs; `standard-compare-raw/measurements.json` independently records/verifies that session’s identities. All failed/setup attempts remain separate: two test fixture setup failures (noncanonical package name, then relative std path), corrected absolute path run, and the initial quick runner’s unsupported absolute granted path. No algorithm assertion was weakened.

Frozen participant SHA-256:

- `before`: `02ca6b6fa782182201123e859da582f08597f894986da494190595608feafb62`
- `candidate`: `7f4e45d1faf4962c46d34b8ce5ba0bbf77d3fd5d93907a874d52b03b459fcb58`
- `rust`: `34f46c760fcc739816add285f1328cd3fb8eef465143303f6c96ef7349dc8cb3`
- `cli`: `90dfbd42bf3380d27041c1634e769a128e4bf0f579a9b98adc937071f08715e9`

Pinned `compress.rs` SHA-256 `d30733cc1104432093193002f0149b2450fd8f87a375e66b5efeb3f63ea858d7`; `test.js` SHA-256 `424371dd25f3fa3f779789c823a05dbe90d2eddbc2a3ca3c5e4319252c572797`; both matched BACKLOG-10-REFERENCES at launch.

## Adaptations and remaining route

Reference seams: `rust/src/benchmarks/compress.rs:890-919` encoder, `:963-998` decoder, `:1006-1028` prepare/run/equality; `test.js:294-305` independent checksums. Byte-view reference counting replaces Rust’s borrowed Vec lookup/move. Hirsel Map[Bytes,u64] remains distinct from Rust HashMap<Vec<u8>,...> and cannot request 4096 capacity through the current public surface. List physical clone capacity/allocator classes and checked u64 indices/narrowing differ from Vec/usize; malformed-length failure uses `failure.check`. These are the existing language adaptations; no new algorithm deviation is introduced.

The remaining encode instruction ratio is about 1.38; profiles point to full-key hashing/comparison and map setup. The concrete next route is a separately owned public map-capacity API followed by the minimal `map.empty(); reserve(4096)` initialization probe and a fresh paired campaign; that API is outside this lane. Decode stays faithful and unmodified; a reserve-after-clone cannot remove the existing allocation, and a new runtime clone-with-capacity API is explicitly out of scope. No mandatory functionality remains unfinished.

## Reference license

The test oracle and port follow LangArena under its MIT notice:

```text
Copyright (c) 2026 'Konstantin Makarchev'

MIT License

Permission is hereby granted, free of charge, to any person obtaining
a copy of this software and associated documentation files (the
"Software"), to deal in the Software without restriction, including
without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to
permit persons to whom the Software is furnished to do so, subject to
the following conditions:

The above copyright notice and this permission notice shall be
included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE
LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```
