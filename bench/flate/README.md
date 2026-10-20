# DEFLATE comparison

`prepare OUTPUT ZLIB_CLONE` creates deterministic words (100,000 bytes), noise
(70,000 bytes, Python Random seed 42), and runs (200,000 bytes) inputs. Each
participant compresses the input 20 times at level 1, 6, or 9 with raw DEFLATE,
windowBits 15, memLevel 8 and the default strategy, and prints the sum of output
CRC-32 checksums. `bench/micro/run` rejects differing output checksums.

Clone madler/zlib at `09155eaa2f9270dc4ed1fa13e2b4b2613e6e4851` (1.3), and
build its static archive with `./configure --static && make -j2 libz.a` before
preparing the workloads. `reference.c` calls that archive directly.

Build the repository's release CLI and native toolchain, then compile
`OUTPUT/encode.hir` with `hirsel build --release` to seal each Hirsel executable.
To compare implementations, preserve the original std source and point a copy
of `encode.hir.json` at its manifest before compiling `OUTPUT/map`. Compile
`OUTPUT/array` with the changed std source. Both executables must use the same
compiler, runtime, allocator, optimization level and native toolchain.

For each generated participant file, run:

```sh
bench/micro/run OUTPUT/words-6-participants.json --reference zlib --rounds 10 --seed 42 --output OUTPUT/words-6-session.json
```

The runner pins CPU 31, takes the shared timing lock, interleaves participants
with seeded order, records perf instructions and cycles, and saves raw samples.
Instructions decide the comparison; timing ratios apply only within a session.
