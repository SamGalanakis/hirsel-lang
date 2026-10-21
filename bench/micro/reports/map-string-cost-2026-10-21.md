# String counters and ordered-map lookup

The original two-probe and borrowed-key upsert programs are retained. The Rust
reference now also exposes matched get+insert and owned entry controls, so caller
API overhead can be assessed separately from implementation overhead.

The production change keeps SipHash, cached full hashes, tagged ordinal buckets,
linear probing, tombstones and insertion-ordered entries. Lookup tests a matching
live tag before checking for an empty bucket and reads cold-image metadata only
on the linear path. The same probe is emitted into get/edit/remove callers with
three plain block arguments, avoiding a separate helper/tuple ABI round trip.
Standalone registered native probe helpers remain available.

A single String/Bytes unique copy skips the general graph bookkeeping after the
existing bounded-arena precharge. It uses the existing whole-block or compact-view
copy, including explicit destination, count, capacity and tracking behavior.
Other descriptors, aggregates and Shared roots retain the general walker.
No caller copy expression is elided, and insert still receives and drops its
owned incoming key in the original order.

## Prior art and differences

- Rust `a7c37f9f0ee5e93396106e9de20f4a86b4154e63`,
  `library/alloc/src/slice.rs:446–459`: allocate once, then copy the byte interval.
  Hirsel retains whole-owner capacity semantics and sequence headers, compact
  slices, explicit heap/arena destinations, diagnostic checks and arena charging.
- The same Rust commit, `library/std/src/collections/hash/map.rs:1012,1296,1332`:
  entry, borrowed get_mut and get+insert controls. The existing get_mut control
  clones only on misses; owned entry and get+insert clone on every selection.
  Hirsel upsert borrows its key and copies only on a miss.
- hashbrown 0.16.1 `1876e4f02708b93903d55ef598f68e82a826518f`,
  `src/raw/mod.rs:2049–2084`: candidate tags precede empty termination.
  Hirsel uses scalar 32-bit tags plus ordinal words, rather than SIMD Swiss
  control groups; ordinal zero excludes tombstones and empty buckets.
- hashbrown `src/map.rs:5241–5270,5626–5673`: collision/removal and entry
  update/remove/miss vectors. Hirsel additionally checks insertion order,
  growth, tombstones, reinsertion, aliases and nonzero UTF-8 slice offsets.

## Evidence contract

The baseline is `0a0d9aed835ce1808f18342a139835935187df05`. Its compiler, std
image/source, runtime archives, exact benchmark sources and ELF hashes are frozen.
The accepted programs include vocabulary creation, PRNG selection, key copy,
validation checksum, cleanup and startup. All participants use the same budget,
CPU31, ten seeded interleaved rounds, and perf total/user instructions and cycles.
Only within-session time ratios are meaningful; no busy samples are excluded.

Evidence is retained outside Git at `/tmp/map-string-cost-evidence` with the
build/run scripts and logs indexed by `/workspace/code/hirsel-briefs/map-string-cost`.
The original whole-key sizes are 2M selections, 1M u64 inserts/gets and 20k
removals. Diagnostic profiles distinguish whole/sliced keys and mostly-hit
(2M selections) / mostly-miss (1,000 selections per fresh vocabulary, 100 epochs)
inputs. Profile percentages are sampled attribution, not the performance verdict.

## Validation

The scoped RC unit target executes 9 tests. Focused map pipeline tests execute
21 tests (20 before plus the forced full-hash collision control), with five
pre-existing native-only ignores. Copy tests execute 18 (17 before plus the
bounded UTF-8 view matrix). The collision fixture makes every live key's full
hash equal to the tombstone sentinel tag; it checks updates, removal, missing
lookup and reinsertion order through the heap checker and failure campaign.

Seven selected corpus cases pass expected execution and canonical formatting.
The same seven pass the filtered native smoke target (four configurations,
real-Io record/replay and Cranelift) and the filtered LLVM/Cranelift parity target.
The new UTF-8 fixture retains an alias through a copied map, replaces a key,
updates through an equal whole String, removes/reinserts through a slice, and
checks both the original holder and the complete resulting iteration order.
Existing replaced-value controls retain their drop assertions.

Initial setup failures are retained: unsupported `root//:*` discovery (use
`root//:`), obsolete precheck filename, incomplete toolchain copy on ENOSPC,
interrupted unused std-object build and stale working directory, a compile-time
missing mutable borrow during probe movement, a source-size upload mismatch
before the fixture freeze, and the new native fixture's missing std dependency
manifest. None is counted as executed coverage. Accepted baseline sessions
were preserved and never repeated for setup recovery.

## Results and disposition

Instruction ratios to the retained original Rust control, from four separate
seeded ten-round sessions containing before, candidate 1, candidate 2 and Rust
together. “After” below is candidate 2. Every one of the 200 samples exited
successfully and matched its session's stdout checksum.

| Workload | Before total / user | After total / user | Instruction change | After cycles / Rust |
|---|---:|---:|---:|---:|
| `map_str` | 4.8590 / 4.8615 | 3.7450 / 3.7419 | -22.93% | 3.0822 |
| `map_str_upsert` | 1.4330 / 1.4183 | 1.2653 / 1.2497 | -11.70% | 1.2909 |
| `map_u64` | 1.1070 / 1.0415 | 1.0877 / 1.0196 | -1.75% | 0.9829 |
| `map_remove` | 1.7765 / 1.1985 | 1.7100 / 1.1169 | -3.75% | 2.5207 |

The upsert acceptance goal of **≤1.2× Rust remains INCOMPLETE/NO-GO**:
after is 1.2653× total and 1.2497× user. Adopt the measured production
improvement; do not claim parity. The unchanged u64/removal rows improve
1.75%/3.75% in total instructions. Removal includes substantial startup/kernel
work (1.7100× total versus 1.1169× user); it is not an isolated removal-loop
cost or a cross-session timing claim.

The retained two-probe String row remains **3.7450× original Rust**, or
**1.3885× the matched Rust get+insert control**. Within that same session,
matched Rust get+insert is 2.6972× original Rust and owned entry is 1.8139×.
In the upsert session, those controls are 2.6985× and 1.8145×. This separates
unconditional caller key cloning/two probes from implementation overhead.
The original Hirsel caller still evaluates `unique w` on every update; its
allocation/copy and destruction are preserved. Upsert still copies only misses.

Candidate 1 (tag-first lookup plus the slice copy fast path) is 3.8224× for
two-probe and 1.4205× for upsert. Emitting the same probe directly into callers
then reaches 3.7450×/1.2653×. These staged results are evidence for the two cost
sources, not a claim that each source accounts for an isolated percentage.

Sixteen before/after cycle profiles cover both APIs, whole/sliced keys and
mostly-hit/mostly-miss selections. Before whole two-probe hits attribute 11.18%
to `copy_graph` (sliced 10.52%); that general walk disappears from the after
String profiles. Before upsert hits attribute 40.38% to the separate probe
(sliced 43.04%); after attributes 43.73%/46.99% to the fused upsert body.
These sampled percentages change their denominator and do not measure speedup.
Key generation and final validation remain included, particularly in mostly-miss
profiles. SipHash and byte/key framing are unchanged.

Smallest retained performance repro: use the unchanged
`programs/extended/map_str_upsert.hir`, 2,000,000 selections, one epoch over
4,096 prebuilt Strings, against Rust's original borrowed get_mut control.
`final-map_str_upsert.json` freezes all commands and inputs. The two-probe
residual is retained by `programs/map_str.hir` at the same size and its matched
Rust two-probe participant. No thresholds, workloads, allocator policy or
resource caps were changed to obtain these results.

The ignored physical String hash-seed law initially stopped before linking:
its harness assumed whole-program emission returned one object, whereas the
current emitter also appends program glue. The harness now finds exports across
all emitted objects and links every object. Its original SipHash framing,
three-seed bucket variation, ordered entry and zero-leak assertions remain
unchanged. The exact String selector passes on LLVM/Cranelift, x86_64/AArch64,
O0/O3, with reuse on/off. Additional native Set order and deep-key source-failure
attribution selectors pass (two tests).

The two other seed-law callers of the repaired harness (u64 and composite keys)
also pass. All five formerly ignored native map tests were therefore executed
explicitly: three seed laws, Set order and deep-key failure attribution. Each
seed law covers 16 native executable configurations and three runtime seeds
(48 seeded executions); the C oracle independently checks cached hashes,
placement variation, iteration order and exit memory balance.
