# Whole-buffer Zstandard benchmark

Build an old executable from `decode.hir` before changing std, then build the
new one and references on the same input:

```sh
bench/zstd/build --input /tmp/http.zst --output /tmp/zstd-old
cp /tmp/zstd-old/new /tmp/old-zstd
# Apply the decoder change.
bench/zstd/build --input /tmp/http.zst --output /tmp/zstd-new --old /tmp/old-zstd
bench/micro/run /tmp/zstd-new/participants.json --output /tmp/http-session.json --seed 42
```

The build helper pool-builds the release compiler/runtime and invokes each
compiler separately under `tools/capped`. It requires Zig 0.16, Rust, a C
compiler, libzstd.so.1 and libxxhash.so.0. No packages are installed. The Rust
participant is explicitly a **libzstd FFI wrapper**, not an independent Rust
codec; it controls wrapper/allocation overhead against the C participant.
Zig uses `std.compress.zstd`. Hirsel uses ordinary std source.

Every process reads the compressed file once, decodes it ten times, computes
XXH64 over the first full output, and adds every output's length and its first
and last byte to a wrapping sum. The decoder is pure and receives identical
input on all ten iterations. The counted process
includes startup, input read, allocation, decode, hashing and cleanup. Inputs
must decode to at most 20,000,000 bytes and require at most a 20,000,000-byte
window. The harness requires all printed digests to agree and collects ten
seeded interleaved rounds with sibling-load snapshots. Process throughput is
`10 * uncompressed_bytes / median_task_clock_seconds / 1e6` MB/s; it includes
one full hash and is not a decode-only measurement.

The original HTTP input was created by repeating Zstandard's
`tests/golden-compression/http` to exactly 10,000,000 bytes, then compressing
at level 3 with libzstd 1.5.5. For a heterogeneous sample, concatenate the first
262,144 bytes of each of the twelve files in `silesia.zip`, in sorted filename
order, and compress the result at level 3. The corpus is downloaded separately
from <https://sun.aei.polsl.pl/~sdeor/corpus/silesia.zip>; its bytes are not
redistributed here. Preserve input hashes and library/toolchain versions with
session results. Compare times only inside one session, with old and new
executables both present; the instruction ratio is the verdict.
