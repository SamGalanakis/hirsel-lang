# Compression prior-art realignment

References were cloned and checked at these exact commits:

- golang/go `ebe2feefc400f813aefdc8a036e66d2619da6649`:
  `src/compress/flate/reader_test.go:16-24`, `flate_test.go:20-80`,
  `src/compress/gzip/gunzip_test.go:529-579`.
- madler/zlib `09155eaa2f9270dc4ed1fa13e2b4b2613e6e4851` (1.3):
  `contrib/puff/puff.c:235-255,340-379`,
  `deflate.c:155-164,187-211`.
- facebook/zstd `49cf51799ea051457f57243527aa2058f999f005`:
  `lib/common/fse_decompress.c:59-160`,
  `lib/common/entropy_common.c:42-195`.

The FSE constructor now accepts a normalized distribution with one symbol
occupying every state, as libzstd does. Corpus vectors exercise compressed
single-symbol LL, OF, ML and all-three tables, each with a truncated twin.
The exact reference library accepts all four frames as `aaaa`. A compressed
Huffman-weight table with one symbol consumes zero state-update bits forever;
libzstd rejects output overflow and Hirsel returns Corrupt at its 255-weight
bound. Accepting that table does not make its unterminating weight stream valid.

All nine audit-listed inflate vectors are now present: TestNlitOutOfRange,
seven huffmanDecoder.init bit-length sets, and the gzip original FEXTRA stream
at all 40 proper nonempty prefixes, plus its complete decode twin. The original
Huffman bit-length sets are embedded in public dynamic-block inputs; the last
nonzero length moves to symbol 256 (EOB), and unused symbols are zero-padded.
This preserves completeness and over/undersubscription without exporting a
private table builder. The valid Issue6255 set decodes three literal symbols.
The InvalidEncoding set assigns its one-bit zero code to EOB and supplies an
invalid one bit. zlib 1.3 verifies all these raw-block accept/reject outcomes.

Corpus program counts: flate 18 -> 22; zstd 8 -> 9. The nine listed upstream
vectors go from 0/9 to 9/9. Existing TestStreams (28/28), gzip (15/15) and zlib
(14/14) vector coverage is preserved.

Deviations retained or introduced, with reasons:

- Resumable bounded decoding and strict trailing-data errors serve Hirsel's
  caller-selected output bounds and complete-input contract. Preset zlib
  dictionaries remain Header errors because that API has no dictionary input.
- Huffman counts/symbols follow puff; Go's completeness rule and error policy
  remain. A lazy direct table handles short codes after 64 symbols, and the
  canonical walk handles the rest without Go's secondary link tables. This
  existing hybrid is retained; this change measures the encoder only.
- The zlib encoder keeps the default strategy, windowBits 15, memLevel 8,
  level-specific match policy and exact output. Hirsel index storage is u64
  rather than zlib's 16-bit Pos; this matches the language's list indexing and
  checked arithmetic. The fixed-length Array uses `[0u64; 32768]`; List is not a repeat-literal target
  and building a 32,768-entry List in comptime exceeds the evaluation budget.
  Array preserves zlib's flat layout without introducing a compiler change.
  Head allocation is delayed until first string insertion
  so unused/level-0 encoders avoid it. prev remains incrementally initialized
  because unread chain positions are never visited.
- Go's internal Huffman tests are encoded as blocks, with zero padding and EOB
  relocation, to test through the public decoder. Error categories are pinned;
  offsets differ from Go's internal bool-returning init interface.
- Single-symbol FSE tables retain libzstd's state layout. Whole-buffer zstd
  output/window bounds, mandatory checksum validation and dictionary APIs
  remain the existing documented Hirsel contract.

The deferred large zstd fixtures and missing-dictionary fixture belong to the
separate harness/performance work described in the audit and are outside this
step's assigned two compression changes.

The final benchmark uses the same release compiler and runtime for both Hirsel
variants, zlib 1.3 compiled with cc -O3, three inputs and levels 1, 6 and 9. Each
sample compresses 20 times and consumes output with CRC-32; process startup,
input loading, compression and the checksum are all counted. CPU 31, seed 42,
ten shuffled rounds, perf instructions plus cycles. Every output agrees. Raw
medians and executable/input hashes are in `/workspace/code/hirsel-briefs/pa-compress/repo-evidence/bench/flate/results.json`; preparation and
measurement commands are in `README.md`.

| input | level | Map / zlib instructions | Array / zlib instructions | Array / Map |
|---|---:|---:|---:|---:|
| words | 1 | 4.242 | 4.332 | 1.021 |
| words | 6 | 13.796 | 3.587 | 0.260 |
| words | 9 | 13.790 | 3.586 | 0.260 |
| noise | 1 | 9.960 | 2.859 | 0.287 |
| noise | 6 | 8.917 | 2.630 | 0.295 |
| noise | 9 | 8.900 | 2.631 | 0.296 |
| runs | 1 | 3.712 | 3.972 | 1.070 |
| runs | 6 | 13.050 | 3.456 | 0.265 |
| runs | 9 | 12.973 | 3.441 | 0.265 |

The array removes about 70-74% of instructions at levels 6 and 9 and on noisy
input. On these highly repetitive level-1 words and runs it costs about 2% and
7% more instructions: allocating and sliding the whole table exceeds the map's
small occupied set there. The zlib layout is retained because it removes the
large map overhead across the other measured workloads and matches the reference
structure. It remains about 2.6-4.3 times zlib's instruction count; this work
makes no claim of zlib speed parity. Times are recorded only for within-session
ratios; instruction counts decide the change.

Validation: 5/5 added corpus programs (including all nine
missing inflate vectors, 40 gzip prefixes and the single-symbol FSE vectors),
7/7 blocks checked with the exact zlib 1.3 C oracle, four valid FSE frames plus
the overflowing weight stream checked with the exact cited libzstd revision,
and the existing native large DEFLATE golden fixture at all levels 0-9, with
one-shot and uneven-chunk streaming compression and decode-back checks.

The remaining affected corpus closure also passed: 144/144 existing programs,
for 149/149 including the additions. All 129 active driver stdlib tests passed.
The native large fixture ran explicitly because its Rust harness is ignored
in ordinary package test runs.

All 61 active CLI example tests also passed, including gRPC and Datastar
package tests, HTTP servers, and tasks package and native end-to-end tests.
The three targeted Buck2 invocations passed 76 test targets in total.
