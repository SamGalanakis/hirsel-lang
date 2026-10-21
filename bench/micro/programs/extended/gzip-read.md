`gzip_read.hir` decodes a gzip member containing 1 MiB of `a` bytes, checks
both the output length and sum, and requests either one or 4096 bytes per
`flate.read`. The 1052-byte compressed fixture is Python/zlib's level-9
`gzip.compress(b'a' * 1048576, mtime=0)`. The input reader holds up to 64 KiB.
Both cases expose repeated inspection of unread compressed bytes; the one-byte
case is the regression workload. Each read gets a fresh bounded output list.

Run from the repo root (Linux perf counters and CPU 31 are required):

```sh
bench/micro/programs/extended/gzip-read --before <original-commit> --output /tmp/gzip-read
```

The runner extracts the original stdlib from git, compiles the same source
against both libraries with the same compiler and release settings, and runs
ten seeded interleaved rounds per chunk size through `bench/micro/run`. It
uses the shared timing lock, records cycles and instructions, checks identical
stdout checksums, and writes each session's raw samples and ratios. This
before/after workload has its own runner rather than a Rust comparison pair.

Measured on 2026-10-21 against stdlib revision `49b74942`, with the same compiler,
LLVM release output, CPU 31, ten rounds and order seed 20261021 per session.
The program produced `1048576 101711872` in every sample.

| Bytes requested per read | Before instructions (median) | After instructions (median) | After/before instructions | After/before CPU time |
| --- | ---: | ---: | ---: | ---: |
| 1 | 9,321,426,538.5 | 918,272,708 | 0.0985 | 0.1091 |
| 4096 | 53,647,687.5 | 53,458,816.5 | 0.9965 | 1.0104 |

Instruction counts are the verdict: one-byte reads use 90.1% fewer instructions
(10.15× reduction), and 4 KiB reads are effectively unchanged. CPU-time ratios
are within-session only: 95% bootstrap intervals are [0.1049, 0.1136] and
[0.9713, 1.1991], respectively. Cycle ratios are 0.1118 and 1.0143. Sessions:
`b1dd6f43-c791-4f88-be63-d85fff4567d8` (1) and `0bd309d1-8fa1-4a99-8352-7140bef0d50a` (4096).

The compressed fixture is 1052 bytes, expanding to 1 MiB; these are decoded
read sizes. This micro measures that fixture, rather than claiming a throughput
result for incompressible input. The implementation removes whole-buffer
snapshots for any input size: peek creates a slice, consume advances an offset,
and refill alone compacts/freezes bytes. Decoder history compaction remains
amortized over the output, so repeated small reads no longer copy the buffered
input on every call.
