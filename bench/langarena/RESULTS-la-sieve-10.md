# Sieve: one backing allocation per iteration

Reserving `limit + 1` byte slots before the existing push loop reduces production
retired instructions by **8.56%** in ten seeded, interleaved before/reserve/bound/Rust
rounds. The byte initialization, composite writes, inclusive bound, `p*p` starting
points, odd final scan and warmup/checksum boundaries are preserved. No filled-list
API or other lane's implementation is used. A precomputed integer marking bound
was evaluated and declined: its difference from reserve-only is below the
process-counter variation, and deterministic profiling shows additional work.

## Scope and verdict

Baseline: current origin/main `d5b594d8250ad3c306e3caca6b68aa7cb5b754ba`.
Reference: LangArena `3c9f84ddaeec834c0631747cb789e8cfd602f993`, unchanged
`rust/src/benchmarks/sieve.rs:21–57`; `test.js:90–95` supplies limit 1000,
three measured iterations and checksum 4660 (one warmup step).
Both reference files matched BACKLOG-10-REFERENCES.json at launch:

- sieve.rs SHA-256: `04f12246ff8761a2418eca8a539aa62b579b1956f30b82204e0319a04debd4ca`.
- test.js SHA-256: `424371dd25f3fa3f779789c823a05dbe90d2eddbc2a3ca3c5e4319252c572797`.

The production case is limit 3,000,000, 270 timed + 54 warmup iterations,
checksum 1042248060. Instructions cover the complete process, including startup,
JSON/config parsing, preparation, warmup, marking, final scanning and checksum.

| Participant | Production instructions (median, 10 rounds) | Relative to before |
| --- | ---: | ---: |
| Frozen before | 31,035,143,068.5 | 1.000000 |
| Reserve only (selected) | 28,378,607,469.0 | 0.914402 |
| Reserve + integer bound (declined) | 28,374,296,953.0 | 0.914263 |
| Unchanged upstream Rust | 19,888,811,030.5 | 0.640848 |

All 80 runs passed: ten rounds × four participants × two original configurations.
The quick case is correctness evidence only; its process startup dominates and
its instruction differences support no performance claim. Seed 20261022,
CPU12 affinity, ordinary free CPU, `nice -n 10`, `tools/capped`, process-tree
`perf stat instructions,cycles`; all counters ran at 100%. CPU31's timing lock
was occupied at entry, so this session took no CPU lock and claims no before/after
wall-clock speedup. No CPU lock was held while waiting for another.

A separate standard candidate/Rust session acquired CPU31's exclusive timing
lock immediately after builds. Ten seeded rounds (20261021), 20/20 passing,
100% instructions/cycles counter coverage; the final candidate/Rust instruction
ratio is 1.427 and timed-interval ratio is 1.479 (2.066892 s / 1.397500 s).
Those ratios apply only within that session; they do not compare before/after
wall time or historical RESULTS.md numbers. SMT sibling snapshots and all raw
counters are retained with the standard session. No other task ran.

## Initialization, marking and allocation diagnosis

Callgrind 3.22.0, same frozen release executables, CPU13, one fresh production-size
step (warmup=0, iterations=1, checksum=3216815). This diagnostic changes only the
iteration counts; it supplies phase attribution, not a timing ranking. Instruction
addresses in the owning `step` symbol separate initialization, marking and the odd
scan; initialization includes its callees. The address ranges and raw profiles
are retained. Runtime startup and cleanup are excluded from this phase table.

| Phase | Before instructions | Reserve instructions |
| --- | ---: | ---: |
| Allocation + byte initialization | 22,737,734 | 21,000,302 |
| Marking from p*p | 46,282,484 | 46,282,484 |
| Odd final scan | 15,650,442 | 15,650,441 |

The production-size step calls the byte-list growth allocator 19 times before
(1 initial + 18 growth/copy operations), versus one reserve allocation and no
subsequent growth in the selected version. The complete process Callgrind totals
are 85,999,218 before and 84,260,362 reserve instructions. Its allocation/copy
coverage differs from hardware PMU counts; these are separate observations,
not interchangeable ratios. Composite-store execution counts are identical:
6,609,233 in each single production step (including duplicate composite writes).

A separately linked LLVM measurement-runtime companion attributes allocations to
the actual step's reserve/push site: **28 → 4 backing allocations** for four
limit-1000 steps (7 → 1 per iteration; 6 → 0 growth allocations). It omits flag
printing and separates runtime startup/output allocation sites. This is logical
Hirsel backing-block accounting, not libc allocator or RSS accounting. The
production Callgrind call edges cover the larger list separately. Both confirm
fresh allocation and initialization on every iteration.

The restoring integer-square-root experiment reuses the structure already in
`stdlib/std@1/list_block.hir:41–62` (Zig `lib/std/math/sqrt.zig:36–61`, pin
`bff490a23889750de7096d9057c0a432efc929a8`). It matched independent Python
`math.isqrt` on 11,176 values: all 0..1000, power-of-two boundaries, perfect-square
neighbors, u64 extremes and 10,000 seeded random u64 inputs, checking every
`res + one` addition. Its experimental execute guard restricted configurations
to 2..=i64::MAX so length, p*p, multiple+p, n+2 and last+count stay representable.
This guard and helper are **not in the selected kernel**. The private experiment
is retained in `la_sieve_10/bound.hir`. On the same production-sized one-step
Callgrind diagnostic its complete process costs 84,256,722 instructions; small
runtime-launch variation masks tiny kernel changes. On limit1000 the step's
inclusive instruction count is 93,278 vs reserve's 92,690, so there is no proved
loop-bound win. Keep checked p*p, and revisit already-landed filled initialization
only in a later, separately measured change if initialization remains material.

## Correctness and failures

Actual scoped Buck target: `root//:hirsel-cli-test-la_sieve_10`; filter
`la_sieve_10`, five Rust tests, **5 passed before → 5 passed after**, no skips or
filtered tests. The standalone CLI harness keeps its custom `#[path]` registration, with the
private oracle under `tests/support/la_sieve_10/`. Cargo/Buck automatically
discover its `la_sieve_10.rs` entry; no shared main.rs was edited. The final
helper move preserved all tested code except one include_str relative-path
adjustment; semantic greens and the 100 accepted samples remain credited.
A draft sibling modules.rs triggered the shared-registry check; adding main.rs
then caused Cargo duplicate-name discovery. Both packaging drafts were removed
and metadata was regenerated; their setup failures are retained separately.

The three backend tests (H, LLVM, Cranelift) compare every byte, final prime,
prime count and cumulative checksum against an independent trial-division oracle:
all limits 2..=32, 48/49/50, 80/81/82, 120/121/122, 168/169/170 and 1000,
four fresh steps per limit (176 complete flag arrays per backend). This includes
2,3,4,9,25,49 and square boundaries. The complete expected output and equality
results are retained in the evidence sidecar. At 1000, (997+168)*4=4660.

Unsupported configurations are documented independently: limit0 fails with
MEM-14 at the index1 store; limit1 retains flags00 and the historical last=2,
count=1 seed, yielding sum12 for four steps. Neither behavior nor supported
scoring was changed. This private benchmark is not a generic all-u64 sieve API;
the existing allocation and checked arithmetic failures remain in effect.

Preserved setup attempts: before1 built no tests (Rust partial move); before2
ran four (1 pass, 3 manifest-identifier failures); before3 ran four (0 pass,
4 unsupported String += setup failures); before4 ran five (3 backend passes,
2 failures: missing link directory and an incorrect bounds-error assertion);
before5 ran five (3 backend passes, 2 failures: using a fixture launcher instead
of a standalone launcher for stdout grants, and mistakenly expecting limit1 to
fail). Before6 and after each pass all five. The initial Callgrind attempt
failed because an absolute config path was outside the virtual read grant;
the corrected relative path passed. The missing local rustfmt shortcut was
replaced by the pinned rustup formatter. Every failed/setup log remains separate;
none supplies a ranking. No compiler/runtime implementation failure remains.

## Reproduction and identities

`la_sieve_10/evidence.jsonl` retains all four-participant full process outputs,
per-round raw PMU counts, seed, affinity, exact argv, frozen source manifests,
compiler/runtime/toolchain and binary SHA-256 identities, allocation-site reports,
full expected oracle output, and the separate standard candidate/Rust samples.
All consumed inputs were hashed before consumers; variant packages, runtime,
compiler and binaries were reverified afterwards. The final source adds only
comments beyond the measured reserve-only variant. External detailed logs,
Callgrind files and the standard runner's entire directory are retained under
`/workspace/code/hirsel-briefs/la-sieve-10/session-01`.

Build the before harness from d5b594d8 and the candidate with
`python3 bench/langarena/build.py`; retain each executable before switching trees.
For the declined experiment, copy the harness into an owned package, replace
only sieve.hir with `la_sieve_10/bound.hir`, and point its std dependency at the
same frozen stdlib. Build unchanged upstream Rust with `cargo build --release
--locked` at the pin above. Freeze manifests, grants, compiler, runtime and sources.
Then replay the ordinary-CPU instruction comparison:

```sh
python3 bench/langarena/la_sieve_10/measure.py \
  --before /path/to/before --after /path/to/after --bound /path/to/bound \
  --rust /path/to/unchanged-rust --grants bench/langarena/build/grants.json \
  --output /tmp/la-sieve-10-replay --cpu 12
.buck2/bin/buck2 test root//:hirsel-cli-test-la_sieve_10 -- --test-arg=la_sieve_10
```

The separate locked standard comparison ran exactly:

```sh
python3 bench/langarena/compare.py --upstream /tmp/ref-langarena \
  --filter Etc::Sieve --rounds 10 --session la-sieve-10 \
  --output bench/langarena/RESULTS-la-sieve-10.md
```

## Adaptations and license

The selected port uses Hirsel's public `List[u8]` reserve-plus-push operations
instead of Rust `vec![1u8; limit+1]`; every byte is still initialized in each
measured step. Indices use u64, marking retains the existing checked p*p loop
instead of Rust's floating sqrt bound, and checksum truncation/wrapping follows
the existing u32 state. The tests inject observation after scanning into a copy
of the actual shipped step; they do not substitute a second sieve. The rejected
bound uses exact integer sqrt; its supported-domain guard was experimental only.
LangArena's full MIT notice is retained in bench/langarena/LICENSE and below;
Zig's existing attribution is retained in third-party/NOTICES.md.

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
