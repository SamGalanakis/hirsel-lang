# XXH64 stripe update

The update structure is ported from [cespare/xxhash](https://github.com/cespare/xxhash/tree/ab37246c889f9db16b606fda1c232d659df9271d)
(MIT), cloned for this change at that revision:

- `xxhash.go:72-109`, `Digest.Write`: account for input once, fill a pending
  partial stripe, process complete input stripes directly, then buffer the tail.
- `xxhash_other.go:64-75`, portable `writeBlocks`: keep four accumulators local,
  read four little-endian words per 32-byte stripe, and write accumulators back
  after the loop.
- `xxhash_test.go:13-67`, `TestAll`: all nine seeded/unseeded golden rows and
  every chunk size from 1 through the input length, with one size for empty input.

`stdlib/std@1/xxhash64.hir` follows that sequence for both `Bytes` and `List[u8]`.
Only partial stripes enter `State.mem`. Both direct loops and the buffered
stripe use the same four-word `stripe` kernel. State layout and public APIs
are unchanged.

## Adaptations and deviations

- Separate word readers and loops for Bytes and List, over the same stripe
  kernel: Hirsel has distinct storage types; converting a List to Bytes would
  allocate and copy, violating the existing no-allocation API.
- Explicit little-endian byte assembly replaces Go's `binary.LittleEndian.Uint64`;
  Hirsel has no equivalent raw word-load primitive. Each input byte is read
  once directly, without the old 32-byte round trip through `mem`.
- Bounded loops copy the partial stripe and tail because neither input type has
  a bulk copy into an Inline array. They touch at most 31 input bytes each.
- Four accumulators travel as `Inline[u64, 4]` through the shared kernel rather
  than four Go struct fields, preserving the existing opaque State layout.
- Index ranges replace Go subslices, and the block functions return an absolute
  tail offset. This also preserves the nonzero List suffix offset without copying.
  The remaining-length comparison avoids addition overflow.
- `update_list` retains its existing empty-update rule for starts at or beyond
  the List length. Go has no corresponding List entry point.
- The existing `digest` remains new/update/finish instead of porting Go's
  separate optimized `Sum64`; this change targets Write/writeBlocks, and the
  common path now benefits from direct stripes too. `finish` still consumes
  State, as required by the existing Hirsel ownership API.
- Go's Write count/error return, WriteString, Reset, byte-valued Sum and state
  marshaling are not added: they are outside the existing public API. String
  bytes use `update`; no durable format is introduced.
- The Rust-hosted TestAll fixture uses one empty byte update for empty golden inputs (upstream's
  testDigest makes no calls), explicitly checking the no-op behavior. It also
  checks hex rendering for the six unseeded rows.

## Coverage

The original two corpus programs retain 73 digest assertions: 48 over Bytes
(36 streaming checks, 12 one-shot/hex checks) and 25 List suffix checks.
Two new Rust-hosted fixtures add 456 assertions: 154 over Bytes (142 streaming
checks and the same 12 one-shot/hex checks), plus 302 List/mixed checks.
The new List fixture includes the two previously absent TestAll rows (`as`
and `asd`), both Bytes-to-List and List-to-Bytes updates, empty updates, and
nonzero suffix offsets. Ten additional fixed binary vectors use bytes 00..n-1
at lengths 96, 97, 127, 128 and 129, each with seeds zero and UINT64_MAX;
expected values come from the independent system libxxhash `XXH64`. These
exercise repeated direct stripes, a filled pending stripe followed by more
stripes, exact stripe endings, and 8/4/byte tails.

The two expanded fixtures exceeded the corpus’s 1,000,000-step limit and
therefore run through `it-stdlib/stdlib_xxhash64.rs` with the numeric harness’s
100,000,000-step limit. The small corpus programs stay unchanged. Total
coverage rises from 73 to 529 digest assertions, in two corpus and two Rust tests.

## Validation and measurement

The two new Rust tests passed through
`buck2 test //:hirsel-driver-test-it-stdlib-shard-1 -- --test-arg=xxhash64`
(2 tests executed). All ten selected corpus programs passed: the two original
hash programs and all eight zstd importers. The hash native smoke check passed
22 runs for two cases under four configurations plus record/replay and
Cranelift executable parity. The zstd compressed/golden native checks also
passed 22 runs across the same matrix and parity checks. The original
expanded corpus attempt exceeded
the step limit; those fixtures were moved to Rust as described above.

The benchmark reuses the zstd-perf HTTP workload (1,364 compressed bytes,
10,000,000 decoded bytes): decode ten times, hash the first full output, then
accumulate every output's length and edge bytes. Expected stdout is
`12300404154898655142`. Its HTTP input SHA-256 is
`dc244d534c7a8d8cdae4928fef7a51c93c50df9502b59d37c57441571301c2e6`.

The cited 19.26% absorb profile used commit
`698424a028a24e03f1125ee458d632db56eb5c41`, whose optimized zstd depends on
`bytes.append_from_history`, absent from this checkout's older compiler.
Both benchmark variants therefore use that commit's compiler, runtime and
stdlib; only the after variant's xxhash64 source is replaced by this port.
No compiler/runtime change is part of this patch. Both are release builds.

Measurements use `bench/micro/run`, ten interleaved rounds, seed 42, CPU31,
with perf instructions/cycles (total and user) and task-clock. A separate
997-Hz cycles:u profile repeats each executable 100 times to obtain more
samples than the original 78-sample profile. No cross-session timing ratio
is used. Raw measurement artifacts live outside the repository.


Session `4a2a0f18-e798-4480-a142-9fa3ae634a26`:

| Counter | Before (median) | After (median) | After/before |
|---|---:|---:|---:|
| Instructions | 375,776,249 | 214,631,045 | 0.5712 |
| User instructions | 300,594,795 | 139,629,456 | 0.4645 |
| Cycles | 274,604,094 | 229,373,994 | 0.8353 |
| Task-clock | 65.940 ms | 55.655 ms | 0.8440 |

The instruction verdict is **42.88% fewer total instructions** and 53.55%
fewer user instructions. The within-session task-clock ratio's paired
bootstrap 95% interval is [0.7549, 0.8836]. All 20 stdout hashes match
`48b8fe957b289fe87dcceed96652f7183c0064982c64c03a35d13224f1285150`.

The longer before profile has 5,457 samples: absorb 34.02%, digest 14.26%.
The after profile has 4,127 samples: direct bytes_word 18.54%,
write_bytes_blocks 3.61%, and no absorb symbol. The combined reported hash
share drops from 48.28% to 22.15% (26.13 percentage points). Neither recording
lost samples. The historical 19.26% absorb + 6.38% digest profile had only 78
samples and used a different recording session; these sampled percentages
are attribution, not a cross-session performance ratio. The counted
interleaved instruction comparison above is the verdict.

For completeness, the older unoptimized zstd source in this branch was also
measured in its own ten-round session (`501b6079-960e-4d81-a1c2-cd0e5687f510`):
4.14% fewer total instructions, with decoding still dominating. This is why
the requested profile comparison uses the cited optimized-zstd revision.
