# LangArena JSON: instruction and allocation costs

Current-main baseline `35d4a54a`, October 22, 2026. Full upstream `run.js`:
Generate has 20,000 coordinates and 100 iterations; ParseMapping has 25,000
coordinates and 100 iterations. Both include 20 warmup iterations. Exact checksums
remain 120 and 3,826,684,672. The benchmark port is unchanged.

Ten seeded interleaved rounds (seed 20261022), before/after/upstream Rust on CPU30,
nice 10, through compare.py's Session.sample task runner. All 60 checksum samples
pass; all five PMU events have 100% coverage; frozen source and binary identities
match before/after. Instruction counts include preparation, warmup, startup and
cleanup. No timing lock was taken and no wall-clock speedup is claimed.

| Task | Before instructions | After instructions | Rust instructions | H/R | Reduction |
| --- | ---: | ---: | ---: | ---: | ---: |
| Json::Generate | 25,020,513,914 | 15,658,831,394 | 4,370,213,356 | 5.7252 → 3.5831 | 37.42% |
| Json::ParseMapping | 51,066,872,330 | 42,349,022,290 | 7,688,831,431 | 6.6417 → 5.5079 | 17.07% |

[All 60 counters, coverage, executable identities and allocation totals](RESULTS-la-json-perf-8.json).
Native LLVM release/O3 uses one frozen compiler and runtime; std object images are
disabled. The unchanged Rust binary is upstream LangArena `3c9f84d`, cargo release.
Full compiler/runtime, toolchain/config/source hashes, programs, stdout, raw perf
CSV, perf.data, assembly, failed attempts and test logs remain in the host evidence
directory `la-json-perf-8` under `hirsel-briefs`. The accepted source is the frozen
baseline plus this lane's changes; unrelated main advances during landing are
outside these measurements.

Separate measurement-archive builds count physical Hirsel runtime allocations,
including growth; these are not process-wide malloc counts or Rust allocations.

| Task | Before allocations | After allocations | Live bytes at exit |
| --- | ---: | ---: | ---: |
| Generate | 7,342,639 | 142,639 | 0 → 0 |
| ParseMapping | 54,253,618 | 45,178,618 | 0 → 0 |

Frees equal allocations. Graph-copy totals are unchanged: Generate has one deep
copy, 121 copied blocks and 1,950 copied bytes; ParseMapping has one deep copy,
362 copied blocks and 5,810 copied bytes. These include preparation. Existing
json-owned-results moves remain intact.

## Causes and prior art

1. `decimal.write_unsigned` counted and wrote each digit with u128 division.
   Port itoa `1577ed901354d0d7448ac162328f9dbf5183124c`, src/lib.rs:264-320:
   digit pairs and decimal quads, with a word-sized common path. This alone drops
   Generate from 25.03B to 22.43B instructions in the diagnostic three-round run.
2. Eisel–Lemire normalized one bit per source loop. Match Rust
   `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f`,
   library/core/src/num/imp/dec2flt/lemire.rs:42, using the existing integer
   leading_zeros primitive. No new IR, intrinsic or runtime ABI.
3. Derived float writers made a String and DOM arm, then revalidated its number.
   Follow serde_json `20572a6d066958542d79cdd6cf1a7cd9f0c8e1f8`,
   src/ser.rs:169-181,1716-1723: classify nonfinite values and format finite values
   directly into the destination, whose scalar appender reserves ahead.
4. Typed float readers copied numeric tokens into owned Strings. Follow the
   borrowed numeric conversion structure of serde_json src/de.rs:462-576 while
   sharing Hirsel's exact Eisel–Lemire/DecimalSeq converter over Bytes. Add public
   noalloc f32.parse_bytes/f64.parse_bytes wrappers for CSV and other codecs.
5. Physical probes exposed eager unknown-key set construction, even when all
   fields are known: three empty sequences plus a set record per object. Match
   Rust HashSet::new's delayed allocation (library/std/src/collections/hash/set.rs:
   135-150) with an Option tracker, initialized on the first unknown key. Known
   fields retain their duplicate slots; unknown fields retain the same hashed set.

The original fixed allocation regression (<128 blocks for 1,024 generated
encode/decode pairs) fails with 8,193 blocks on the frozen baseline and with 4,097
after only the scalar changes. Independent probes show the writer uses one
buffer and the decoder accounts for the remaining 4,096. After lazy key tracking,
the unchanged regression passes and the same physical probe records one allocation,
one free, zero copies and zero live bytes. No assertion was weakened.

## Reference deviations and unchanged decisions

- Hirsel's inline formatter accepts any capacity and guarantees BufferTooSmall
  leaves every byte unchanged. It counts first; itoa instead formats backwards
  into a guaranteed-size MaybeUninit buffer. The leading quad is emitted as pairs;
  checked table indexing and ordinary constant /100 replace unchecked loads and
  itoa's handwritten reciprocal divmod100. Wide u128 values use the same quad
  loop rather than itoa's specialized wide helpers; ordinary values take u64.
- Hirsel retains its established shortest text layout and nonfinite strings
  "NaN", "inf", "-inf"; serde_json writes null for nonfinite floats. Exact
  rounding, signed zero, malformed-input errors and full-document error precedence
  are unchanged. Byte parsing rejects malformed/non-ASCII bytes as Invalid.
- Borrow relations and owned-result moves are explicit in Hirsel. Numeric tokens
  remain borrowed until conversion; owned text and decoded graph results stay owned.
- Unlike serde_json, Hirsel rejects duplicate unknown keys too. The lazy tracker
  preserves that policy and the wide-object hashed-set algorithm, rather than
  using a quadratic list or skipping ignored-member validation.
- shortest64 already matches Ryū d2s.c:91-308 and d2s_intrinsics.h:259-281,
  including the common two-digit removal and three multiply-shifts. No speculative
  rewrite was retained. seq_grow already doubles on current main, matching Rust
  RawVec::grow_amortized (library/alloc/src/raw_vec/mod.rs:524-555); no redundant
  runtime growth change was made.

The final lazy tracker costs about 0.6% extra ParseMapping instructions relative
to the eager-set intermediate (42.35B versus 42.10B), while eliminating allocation
for records with only known fields. This task includes unknown name/opts/info
fields, so its allocation total is unchanged by laziness.

## Validation and remaining work

- Decimal corpus: 10/10 selected programs, including all u128 digit boundaries,
  exact capacity and atomic failure; the new fixture also passes frozen baseline.
- JSON corpus: 6/6; public byte parsing: 1/1. After lazy tracking, rerun the three
  affected JSON cases: 3/3.
- Numeric stdlib: 16/16, including Ryū shortest vectors, halfway/subnormal parsing,
  exact fallbacks and all 10,001 short decimal quads.
- Existing JSON stdlib controls: 6/6, including both wide-object duplicate/error
  suites, fuzz oracle, owned-token copies and derived result move/cleanup. The new
  allocation regression passes separately: 1/1; lazy wide-object controls: 2/2.
- Seven exact native cases pass smoke and LLVM/Cranelift parity (14 Buck actions).
  The three affected JSON cases pass parity again after lazy tracking.
- Formatting passes. Setup-only failures (incorrect path/filter forwarding and a
  non-standalone measurement launch) are retained and count as zero executed tests.
  A borrowed-token forwarding error and a take-from-mut error were fixed without
  changing language rules; their failed builds remain in the evidence.

A gap remains: Generate 3.58x Rust and ParseMapping 5.51x. The retained final scalar
profile puts shortest64, append/format call overhead, grammar scans, UTF-8 checks
and duplicate-key tracking next. Minimal triggers are a derived record containing
`x: f64` serialized repeatedly, and a derived Point reader over
`{"x":0.12345678,"y":0.5,"z":0.25,"opts":{"1":[1,true]}}` repeated in an array.
Further removal of those costs needs separate profiling and must preserve the
current policies. Wall-clock comparison remains a follow-up under the host timing
lock rule. No language-rule, TIR/LIR construct, ABI or existing public API changed.
