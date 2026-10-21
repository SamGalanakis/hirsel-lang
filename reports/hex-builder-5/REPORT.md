# Hex append builder

`hex.encode_into(data, mut out)` appends lowercase ASCII directly to the ordinary
`List[u8]` builder. No intermediate String or mutable Bytes bridge is used.
The language rules, IR, runtime ABI, decoder, hash and telemetry call sites are
unchanged. The executable example is in the Hexadecimal codec section of
`docs/spec/std-library.md`.

## Upstream and semantic differences

Ported from Go commit `ebe2feefc400f813aefdc8a036e66d2619da6649`, reused at
`/tmp/ref-go` (identical to the existing pa-text-codecs Go checkout):

- `src/encoding/hex/hex.go:37-61`: EncodedLen, Encode and AppendEncode. Compute
  additional size, grow the destination, then encode high and low nibbles into
  its tail. The private digit writer is shared with the existing `encode`.
- `src/encoding/hex/hex_test.go:20-45`: all seven encDecTests rows, checked with
  empty destinations and the exact `lead` prefix. The existing hex corpus
  retains uppercase decoding and error precedence (`hex_test.go:48-68,92-115`).

Every deviation from the Go append/encode structure:

1. Hirsel checks `len <= u64::MAX / 2` and the complete prefix-plus-additional
   sum before reserve. Overflow fails `TextInvariant`, retaining the existing
   encode contract; Go uses its `int`/slice size rules.
2. `list.reserve` receives total capacity, whereas Go `slices.Grow` receives
   additional capacity. The public operation mutably borrows its builder and
   returns unit instead of returning a resized slice.
3. The nibble loop pushes bytes through existing list operations. Hirsel does
   not expose writable slice tails or a mutable Bytes view. Source uniqueness
   annotations are justified by only pushing immutable digit-table bytes.
4. Existing `encode` shares the private digit writer but does not use the new
   reserve call, preserving its existing allocation/failure path. It still
   consumes its private list through `text.finish_valid`.
5. Added independent exhaustive Python `bytes.hex` and Python hashlib vectors,
   sliced/repeated input, a UTF-8 prefix, a raw invalid-UTF-8 prefix, aliasing and
   explicit unique-output type checks. Decode itself is untouched.

No rollback guarantee is introduced. Resource failures follow normal builder
failure and unwind cleanup, including after a partially completed append.

## Reproduction and identities

Worktree began at compiler/source/runtime commit
`2b0e271fc5e7de39bb38f60e5bb90efec3b1da96`; std is `std@1` from that tree plus this additive change. The report's
measurement data records complete executable and output SHA-256 hashes. LLVM is
the repository-pinned LLVM 21.1.8 toolchain. All Ready builds validate every stage.

Discover targets with `.buck2/bin/buck2 targets 'root//:'` after
`python3 tools/buck2/prepare.py`. Filtered libtest selection is supported on
`root//:hirsel-driver-test-it-stdlib-shard-1`; the first shard runs all matching
cases, as documented by `tools/buck2/test_launcher.sh`.

```sh
.buck2/bin/buck2 test -c hirsel.tests=focus root//:hirsel-driver-test-it-stdlib-shard-1 -- --test-arg=hex_builder_5 --test-arg=hex_is_ordinary_source --test-arg=--nocapture
.buck2/bin/buck2 test -c hirsel.tests=host root//:hirsel-driver-test-it-stdlib-shard-1 -- --test-arg=hex_builder_5::measurement_artifacts --test-arg=--ignored --test-arg=--nocapture
python3 reports/hex-builder-5/measure.py /tmp/hex-builder-5-measure
```

The ignored bounded Rust harness emits native programs; workloads over the
corpus's 1M-step budget never run as corpus cases. The measurement uses 2,048
appends at 16/32/64 bytes, and one 65,536-byte append. Both paths build the same
deterministic input (`(i * 37 + 11) mod 256`), prefix `hex: `, finish the builder,
write every byte and release all values. Rust ports the same Go reserve/nibble
loop with Vec's additional-capacity convention. Setup/finalization are included.
The Rust comparator uses the system allocator; runtime/allocator differences
remain part of the language comparison, not a claim about codec kernels alone.

Ten rounds shuffle all 12 cases with seed `0x48455835`. CPU 31, nice 10,
`perf stat` counts total and user instructions and cycles separately. Allocation
runs use separate instrumented executables and are excluded from perf. No time
claim is made. The collector preserves inherited memory caps or tightens them
to 512 MiB; it never raises them.

## Validation and measurements

Original observations are retained: the first run had one existing hex green
and two fixture syntax reds (`copy`/`result` reserved names); a second attempt
mutated consumed inputs, failed remote upload, and executed zero tests. With
frozen hashes, v1 had five passing tests, one cleanup-fixture ownership rejection,
and one ignored measurement builder. After explicit `copy`/`take` corrections,
the two changed cases passed; the earlier five greens are preserved. Mixed-case
decode compatibility is additionally checked by the exhaustive fixture.

The first conformance selector omitted `conformance/` and executed zero cases;
its failure is retained, not counted as a green. The first measurement collector
also had a harness error: an unconstrained linker worker setup under a 512 MiB
cap. Single-thread linking fixes the harness without raising any cap. The next
collector completed ten rounds but then asserted cleanup on a host budget stop.
That assertion was inconsistent with `docs/spec/determinism.md:256-261` and
`docs/spec/programs.md:127`: budget stops intentionally run no destruction. The
corrected collector preserves the ordinary completion cleanup assertions and
checks the separate stop's exact exit 5, memory-budget observation, and lack of
an exit audit. H's 46-site ordinary-failure cleanup campaign remains unchanged.

Executed final measurements and selector counts follow below.


### Final focused results

Six distinct non-ignored libtests passed across the frozen v1/v2 runs; the
explicit ignored measurement-artifact test then passed when invoked separately.
The changed-case retry ran **2**, not zero, tests. The corrected conformance
selection ran **5 validated / 5 passing / 0 failing / 0 not implemented**.
No whole campaign was run. Existing greens were not rerun just because main
moved. The initial baseline was **1 passed / 2 fixture errors**; the aborted
upload attempt executed **0** tests.

The five lane tests are exhaustive vectors, alias/uniqueness/failure cleanup,
existing hex decode errors, thirteen digest callers, and four text/wire callers
(multipart, telemetry, telemetry_context, x509). Each program compares the
interpreter observation with clean H, LLVM O3 diagnostic and Cranelift diagnostic
execution, with every LIR stage validator enabled. Final H live storage is zero
for every completed program. The cleanup case additionally audits all **46**
failure-capable dynamic sites and asserts one clean-run Watch finalizer; its
clean run allocates/frees **4/4**, peak counted capacity **64 bytes**. The
exhaustive vector run allocates/frees **61/61**, peak **1,600 bytes**.

Native memory-budget controls at 4 KiB for digest workloads and 128 KiB for the
large workload all returned exactly **exit 5 / budget_exceeded memory / audit
unavailable**, without output. These are intentional tooling stops. Ordinary
failure cleanup remains tested separately by the H campaign.

New lane-private paths, listed completely:

- `crates/hirsel-driver/tests/it-stdlib/hex_builder_5.rs`
- `crates/hirsel-driver/tests/fixtures/hex-builder-5-vectors.hir` and `.hir.json`
- `crates/hirsel-driver/tests/fixtures/hex-builder-5-cleanup.hir` and `.hir.json`
- `crates/hirsel-driver/tests/fixtures/hex-builder-5-importers.hir` and `.hir.json`
- `crates/hirsel-driver/tests/fixtures/hex-builder-5-workload.hir` and `.hir.json`
- `reports/hex-builder-5/{REPORT.md,measure.py,comparator.rs}`

`it-stdlib/main.rs` has one appended module registration. Existing
`stdlib_text_wire::hex_is_ordinary_source` is exercised unchanged. `text.hir`
needs no change: this API uses its existing ordinary List builder contract.

### Locked measurements

The accepted session is
`/tmp/hex-builder-5-measure/attempt-20261021T222604692304Z-1967803/`.
It holds the convention lock from `bench/micro/measure.py`,
`/workspace/code/hirsel-briefs/locks/cpu31-timing.lock`, for all warmups, 120
interleaved perf samples and subsequent separate diagnostics. Every completed
sample retains stdout, stderr, perf CSV, exit status and SHA-256; `results.json`
is checkpointed after each sample and before diagnostics. The earlier unlocked
v2 attempt and its surviving output are retained separately and are **not used
for the verdict**. No time-speedup claim is made.

Median hardware counts (whole process, including setup, finish, output and
cleanup; `encode` means existing `encode` plus `text.append`):

| Bytes/chunk | Path | Total instructions | User instructions | Total cycles | User cycles |
|---:|---|---:|---:|---:|---:|
| 16 | encode | 15,233,053.5 | 4,107,541.0 | 15,091,859.0 | 2,347,547.5 |
| 16 | into | 13,237,395.0 | 2,286,866.0 | 14,370,088.5 | 1,777,689.5 |
| 16 | rust | 2,610,221.0 | 1,218,409.5 | 3,240,042.5 | 690,393.0 |
| 32 | encode | 17,464,088.5 | 5,945,961.0 | 16,434,231.5 | 2,993,059.5 |
| 32 | into | 14,722,875.5 | 3,430,386.5 | 14,971,744.0 | 2,065,269.0 |
| 32 | rust | 3,805,924.5 | 2,108,038.5 | 4,136,537.5 | 1,049,218.5 |
| 64 | encode | 21,142,218.0 | 8,996,386.5 | 19,091,046.5 | 4,232,259.0 |
| 64 | into | 17,511,268.0 | 5,716,886.0 | 17,109,781.5 | 3,032,859.0 |
| 64 | rust | 5,878,548.5 | 3,886,456.5 | 5,224,446.0 | 1,711,823.5 |
| 65,536 | encode | 15,434,439.0 | 3,825,254.0 | 16,481,448.5 | 2,545,778.0 |
| 65,536 | into | 14,978,392.0 | 3,758,553.5 | 15,170,077.0 | 2,190,649.5 |
| 65,536 | rust | 4,316,719.0 | 2,646,430.0 | 4,226,544.0 | 1,282,392.5 |

Total-instruction ratios `into/encode` are **0.869, 0.843, 0.828, 0.970** for
16, 32, 64 and 65,536 bytes respectively: gains of **13.1%, 15.7%, 17.2%, 3.0%**.
User-instruction ratios are **0.557, 0.577, 0.635, 0.983**. The single large
buffer has a small gain; process startup dominates its total count. Rust is
cheaper in every workload. This is an append API improvement, not a hashing or
crypto performance claim, and no clients are migrated.

Separate allocation/capacity diagnostics (all completed Hirsel runs free every
counted allocation and end at zero live bytes):

| Bytes/chunk | encode allocs/frees | into allocs/frees | encode peak counted bytes | into peak counted bytes | Rust final Vec capacity |
|---:|---:|---:|---:|---:|---:|
| 16 | 4,111/4,111 | 15/15 | 171,376 | 171,328 | 75,776 |
| 32 | 6,160/6,160 | 16/16 | 293,536 | 293,456 | 141,312 |
| 64 | 8,209/8,209 | 17/17 | 488,704 | 488,560 | 272,384 |
| 65,536 | 30/30 | 16/16 | 375,392 | 212,560 | 131,077 |

The per-class capacity histograms are in the separate allocation files. Peak
counted storage includes input and transient growth, not just final output.
Rust's diagnostic global allocator reports 21/18, 22/19, 23/20 and 22/19
allocation/free events; three process-global stdio/runtime allocations remain
until process exit. The output String and input Vec are explicitly dropped
before that report. These counters are not directly comparable to Hirsel's
counted-block counters. Rust diagnostic hooks are absent from its perf binary.

### Complete identities

Compiler/runtime source: `2b0e271fc5e7de39bb38f60e5bb90efec3b1da96`, plus this
std-only change; runtime ABI **37**, unchanged. Native codegen: LLVM **21.1.8**
and Cranelift **0.136.3** (correctness runs). Rust comparator: rustc **1.99.0**
`b940084d7eb6a299eb4bfeb8e34901bc051e7ac4` (2026-09-28), host
`x86_64-unknown-linux-gnu`, compiler LLVM **23.1.1**, `-O`, system allocator,
one linker worker. Buck2 **6507dd157a6f81a810c48583edf1758dd0c337c5**.

SHA-256:

```text
std@1/hex.hir efcc5fc6724e5dec27e5a3d9a799e906f73f4595f43a3650403e10ffc890a1a1
runtime source/lib.rs e9cc1ec5d41252e12015b17abde3af6060157aaeef11735992d01e2e5d3eb7fa
runtime x86_64 default archive 4d42e186b5c4532ed8f6dbaa2b9798c2d05a39b86dbf755202a046d0de6913b2
runtime x86_64 measurement archive bcf8139cf9a74c24ef7fe9590f3a42cb376d69d377ac900b6dc7209f8afd3148
Rust toolchain-lock.json 8e68b0e2a44a6c7e078b2feff9d3261c3beac5bc5419bdf98cc8cfcebb96d0d3
executable 16-encode b20912c36c0d988dbc13b4e8a6c73997ae24bd7c7fda0f2d7830ceeb49f648fd
executable 16-into 52b32dab3f7032946c2a39f54e17fbddd70c30017cbe61885e89d2b0e6a60252
executable 16-rust 645524ca8964f530ff942cc6fb3107c9d848016e4a7dae25976065268dd3b527
executable 32-encode e0685563ef8656380f5c312db61ce2ba33bb50c69eb003731098f64be003de6c
executable 32-into 1fe0fb96b11af510a21415f5339709abe94b0aadc2f5df733e67cb1fc86743d8
executable 32-rust 645524ca8964f530ff942cc6fb3107c9d848016e4a7dae25976065268dd3b527
executable 64-encode 9b4f46dfaf32bad9d85a897bb0bddf64f8aa2a6a2ccda253aee3f9c7ece8e0fe
executable 64-into eb1094ba4f90981b0540ec84bbd6d10652588ea6debba2e087742b481c05b81e
executable 64-rust 645524ca8964f530ff942cc6fb3107c9d848016e4a7dae25976065268dd3b527
executable 65536-encode 1aadfbd0486ba6f9e87d4b01e96506fcf1d11a2a702bac09b0cd9baea9deb6a1
executable 65536-into cdda6262fdaef7b019cffbbae0e8ee55ff28b3e39bde78fd1a13261a4d2d6846
executable 65536-rust 645524ca8964f530ff942cc6fb3107c9d848016e4a7dae25976065268dd3b527
output 16 bytes/chunk (65541 output bytes) 03a7d774d7119b496f080102d78f76fe85a4e51d45604369366fdd0e67800752
output 32 bytes/chunk (131077 output bytes) 5a64e2ab81aed31ac26a3efce1c27c27c88f9e2978d15df25d8874a493a038f5
output 64 bytes/chunk (262149 output bytes) f281d27c0087c7c5c5ae854bbb69edbad88941ea01fbc8617d4554f6f62be9db
output 65536 bytes/chunk (131077 output bytes) ac8237673e875ecc1aec30ea3ad82fc836ba09f44c5000750c63335c6abfefc3
```
