# CRC slicing-by-8

The IEEE and Castagnoli implementations use Go's portable production structure:
`simplePopulateTable`, `slicingMakeTable`, and `slicingUpdate` in
[`src/hash/crc32/crc32_generic.go:28-90`](https://github.com/golang/go/blob/ebe2feefc400f813aefdc8a036e66d2619da6649/src/hash/crc32/crc32_generic.go#L28).
Reference checkout: Go commit `ebe2feefc400f813aefdc8a036e66d2619da6649`.
The table has eight rows of 256 `u32`s per polynomial. An update of at least
16 bytes enters the stripe loop, which processes eight bytes while more than
eight remain, then finishes with the original byte recurrence.

## Adaptations

- Go's allocated, lazily initialized table is a Hirsel `comptime` constant,
  retaining the table construction loops. This keeps `@noalloc` updates and
  avoids per-state or startup table generation. The 8×256 table is one flat
  `Inline[u32, 2048]`, indexed by `row * 256 + byte`, initialized with
  `[0u32; 2048]`. This avoids the checker cost of spelling 2,048 literal elements.
  Nested row indexing and forwarding the table to a stripe helper copy large
  aggregates in the current native compiler. Keeping the lookups directly in
  the shared IEEE/Castagnoli Bytes kernel avoids those copies. The initial
  nested/helper layout took 16,785,976,365 instructions versus 219,140,110 for
  the original IEEE loop; that layout is not shipped.
- Hirsel's existing `State` stores the complemented remainder. The kernel
  therefore omits Go's complement/uncomplement pairs around the stripe and
  tail loops; the public `new` and `finish` behavior is unchanged.
- Safe byte indexing and widening assemble little-endian words instead of
  Go's `byteorder.LEUint32`. The eight-byte stripe reads its last byte first
  and indexes earlier bytes relative to that checked position. These immutable
  reads commute; this lets LLVM eliminate redundant bounds checks. A standalone
  prototype decreased from 276,537,237 to 150,814,684 instructions with this
  read order (same 32 MiB workload). The table lookups and XORs remain Go's.
- `crc32.update_list` retains Go's original `simpleUpdate` recurrence. Slicing
  a List suffix with an arbitrary start retained bounds checks and measured
  277,376,927 instructions versus 218,537,423 for the byte loop (+26.9%).
  Retaining the byte loop avoids that regression. A start at or beyond the
  end remains an empty update, including `u64::MAX`. It indexes the first
  256 entries of the shared slicing table.
- Go's two long golden rows live in the Rust-side std harness because the
  full split matrix is larger than a corpus program's 1M-step budget.
  The bounded corpus case and its native fixture separately check LLVM/Cranelift agreement.

No polynomial, cutoff, table recurrence, stripe recurrence, or tail behavior
is changed from Go's portable algorithm. Hardware acceleration is outside this
portable-kernel change. The optional Adler-32 unroll is deferred to its separate
vector-completion work.

## Coverage

The existing corpus has the first 31 of Go's 33 golden rows for each polynomial.
The Rust-side harness adds both missing rows from
[`crc32_test.go:71-72`](https://github.com/golang/go/blob/ebe2feefc400f813aefdc8a036e66d2619da6649/src/hash/crc32/crc32_test.go#L71):

| Input | IEEE | Castagnoli |
|---|---|---|
| `"01234567"` repeated 1024 times | `1010dab0` | `8a11661f` |
| `"a"` repeated 1089 times | `772d04d7` | `5a6f5c45` |

Both polynomial harnesses also check 43 binary inputs (lengths 0–33, 63–65,
127–129, and 255–257), one-shot/hex output and streaming splits at 0, 1, 7, 8,
9, 15, 16, 17, halfway, and the end, with an intervening empty update.
Expected values come from a separate bit-at-a-time Rust oracle. A third harness
checks 19 list suffixes following a nonempty checksum, including exact-end,
beyond-end and maximum-integer starts. The corpus case checks another 43
binary prefixes, fragmented updates and a list suffix with an ignored prefix.

## Measurement

A standalone flat 2,048-element `u32` literal checked in 12.60 s and peaked at
1,327.9 MiB. The final flat compile-time builder with `[0u32; 2048]` checked in
1.76 s and peaked at 138.5 MiB. These are single diagnostic runs, not timing
verdicts; they establish that the builder avoids the large-literal checker cost.

Native benchmarks use compiler/runtime revision `c2c54b3b`, LLVM O3 (`build --release`),
and identical application source against std before and after this change.
Each program fills an 8,192-byte payload with `i mod 256`, then performs 4,096
updates on one running checksum (32 MiB total), finally checking that the checksum
is nonzero so the result remains observable. The three cases are IEEE `Bytes`,
Castagnoli `Bytes`, and IEEE `List[u8]` starting at zero. Payload allocation and
process startup are included in the instruction totals.

For example, the IEEE Bytes workload is:

```hirsel
pub fn main() -> Result[(), ()] {
  let mut data: List[u8] = []
  for i in 0u64..8192u64 { list.push(mut data, int.truncate(i)) }
  let payload = bytes.from_list(take data)
  let mut state = std@1/crc32.new()
  for round in 0u64..4096u64 {
    std@1/crc32.update(mut state, payload)
  }
  failure.check(std@1/crc32.finish(take state) != 0, TextInvariant())
  Ok(())
}
```

The Castagnoli case replaces `crc32` with `crc32c`; the list case keeps `data`
and calls `crc32.update_list(mut state, data, 0)` instead. A single interleaved
session runs ten rounds, shuffling all six executables with Python
`random.Random(20261020)`. Each invocation uses
`perf stat -x, -e cycles:u,instructions:u -- <executable>`; the verdict is the
median user instruction count. Cycles and elapsed times are not used as verdicts.

| 32 MiB workload | Before instructions | After instructions | After / before |
|---|---:|---:|---:|
| IEEE Bytes | 219,139,240 | 150,813,833 | 0.688210 |
| Castagnoli Bytes | 219,139,210 | 150,813,932 | 0.688211 |
| IEEE List suffix | 218,536,699 | 218,536,853 | 1.000001 |

Counts are ten-round medians rounded to the nearest instruction. Both Bytes
paths reduce user instructions by 31.18%; the retained List path is effectively
unchanged. No wall-clock speedup is claimed.

## Validation

The driver harness grows from 200 to 203 tests; the three added tests cover
45 inputs per polynomial (the two long golden rows and 43 binary inputs) plus
19 List suffixes. Golden-vector coverage grows from 31/33 to 33/33 per polynomial.
The two original CRC corpus cases remain, with one new cutoff/streaming corpus
case and its native fixture. The selected importer set contains 61 cases,
covering CRC, flate/gzip, ZIP, store and their directly referring programs.

Focused Buck2 commands (run with the actual selected paths in place of
`<importer paths>`) are:

```sh
.buck2/bin/buck2 test -c hirsel.tests=focus \
  //:hirsel-driver-test-it-stdlib-shard-1 -- \
  --timeout 1800 --test-arg stdlib_crc32
.buck2/bin/buck2 test -c hirsel.tests=focus \
  -c 'hirsel.conformance_paths=<importer paths>' \
  -c hirsel.agree_paths=conformance/SEM/SEM-3-crc32-slicing-run.hir \
  -c hirsel.native_filters=crc32-slicing \
  //:gate-conformance-selected-shard-1 //:gate-agree-selected-shard-1 \
  //:gate-native-cranelift-changed -- --timeout 1800
```

Results: all three added driver tests pass; all 61 selected corpus/importer
cases pass, including canonical formatting; both selected corpus/native fixtures
agree under LLVM and Cranelift. The agreement-variant target validates one
selected variant but reports a runtime limit before executing it; it contributes
no execution coverage. The explicit native parity checks provide the engine
comparison. Landing also checks the workspace build, Clippy and formatting.
