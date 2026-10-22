# Recursive Brainfuck: direct tape operations

Current-main baseline `d5b594d8250ad3c306e3caca6b68aa7cb5b754ba`; pinned
LangArena Rust `3c9f84ddaeec834c0631747cb789e8cfd602f993`.
The recursive port uses **38.90% fewer instructions** on the unchanged full
`Brainfuck::Recursion` case. Managed allocations are unchanged.
No wall-clock speedup or published ranking is claimed.

## Change and attribution

`recursive_run` now performs the tape operation directly after matching `Op`,
following upstream `Program::execute` and `Tape::{inc,dec,prev,next}`
(`rust/src/benchmarks/brainfuck_recursion.rs:4–104`). Its recursive Op tree,
recursive calls, byte wrapping, left-at-zero rule, zero-filled tape growth and
print checksum remain intact. Parsing still occurs in every `run`, inside every
warmup and measured iteration. The Array path and its shared `command` body are
unchanged. There is no capacity reservation, flattened interpreter, loop fusion,
precompiled loop, unsafe access, language/IR/runtime change or parsing hoist.

The frozen baseline's LLVM disassembly contains three calls from recursive
execution to `command`: Inc, Dec and Next. Prev and Print were already inlined.
A full-case `perf record -e cycles:u -g` control attributes 63.51% of samples to
`recursive_run`, 36.34% to `command`; parsing and allocation are below the 0.5%
report cutoff. The candidate has no call from `recursive_run` to `command`.
Bounds checks, checked pointer arithmetic and the normal detach/grow operations
remain in the generated code; safety checks were not removed by assumption.

A diagnostic control records actual calls and Op visits, without contributing
performance samples. Pinned Rust and native Hirsel agree on **288,891,814
recursive calls** and **664,453,059 visited Ops** for the full program, final
position 0, complete tape `[10,0,0,0,0,0,0,0]`, emitted bytes
`[90,89,88,87,86,85,84,83,82,81,80,79,78,77,76,75,74,73,72,71,70,69,68,67,66,65,10]`,
and checksum `954437102`. This separates traversal/dispatch from parsing and
verifies that the reduction did not remove recursive work.

The allocation/source control attributes 10,036 allocations to parser builders
and 1,001 to initial tapes (1,000 warmups plus one main iteration). Initial tape
capacity already covers these programs' high-water cells, so growth performs
zero further allocations. Recursive traversal has no observed count operations,
deep copies or detach allocations: the loop body's borrowed reads are free of
counting *in these compiled cases*, as confirmed by source-site counts and
assembly. This is not a general claim about Hirsel borrowed reads.

## Ten-round instruction experiment

Seed `20261022`; ten interleaved rounds, each shuffling frozen before, candidate
and unchanged upstream Rust, 30 successful processes. Full `run.js` configuration:
1,000 exact Hello World warmups, one exact main program, checksum `954437102`.
All participants retain identical preparation/warmup/timed/checksum boundaries.
The two Hirsel packages contain the unchanged production main/harness and a
registry restricted to the two Brainfuck cases; both use native LLVM release,
the same fixed compiler, std, runtime archives and link toolchain. Upstream is
built with pinned Rust 1.99.0, `cargo build --release --locked -j2`.

| Participant | Median instructions | Median user instructions | Instructions / Rust |
| --- | ---: | ---: | ---: |
| Before | 49,341,332,485.5 | 49,259,148,725.5 | 2.0593 |
| Direct tape operations | 30,146,443,003 | 30,094,523,498 | 1.2582 |
| Unchanged Rust | 23,960,671,041.5 | 23,938,237,272.5 | 1.0000 |

Instructions fall 38.9023%; user instructions fall 38.9057%. Whole-process
instruction ranges are before 49,336,945,619–49,343,053,979, after
30,143,587,967–30,157,175,493, Rust 23,959,651,161–23,963,556,343.
Whole-process cycle medians are 17,236,915,270.5 / 10,417,905,086 /
8,178,930,883 respectively; they are supporting observations only.

Affinity is ordinary CPU20 (SMT sibling CPU4); nice level 10; each command uses
`tools/capped` at its default 4G limit. `perf stat` counts instructions, cycles,
user instructions and user cycles in the benchmark child and its threads,
including startup, preparation and warmup. All 120 counter readings have 100%
running coverage. The instruction campaign holds no timing lock. CPU31 is
reserved for another lane; it is never occupied. Lock probes take no wait and
release immediately, within the ten-minute maximum. Contention and SMT activity
make these wall times ineligible for speedup claims.

## Allocation scope

Separate measurement-runtime binaries execute the exact before/after kernels,
1,000 identical warmups, one full main program and an internal checksum assertion.
Regions are disabled by the existing source profiler. These are allocation
controls, not timing binaries: JSON/file/clock harness setup, system malloc,
compiler allocations and upstream Rust allocations are outside their coverage.

| Managed counter | Before | After |
| --- | ---: | ---: |
| Allocations / frees | 11,037 / 11,037 | 11,037 / 11,037 |
| Count increments / decrements | 1,001 / 5,017 | 1,001 / 5,017 |
| Deep copies / COW detaches | 0 / 0 | 0 / 0 |
| Runtime copied bytes | 0 | 0 |
| Peak live bytes / final live bytes | 4,512 / 0 | 4,512 / 0 |

Ten seeded interleaved allocation rounds (20 additional successful processes)
reproduce every counter in the table; all managed allocations are freed.
These source-site ledgers show zero ownership operations in recursive traversal.
Rust is intentionally outside the Hirsel managed-allocation measurement scope.

## Correctness and reference adaptations

Baseline and candidate each pass 3 focused Rust wrappers, **147 Hirsel test
executions** (49 on H, LLVM and Cranelift). The final harness refinement also
passes the same 147 executions. The standalone CLI harness retains its documented automatic registration,
with an explicit path to private cases under `tests/support/`; its Buck selector is `root//:hirsel-cli-test-la_brainfuck_recursion_10`
with `-- --test-arg=cases::`. No skips or zero-selection runs occurred.

The 24 independent Rust-produced rows check every final tape byte, tape length,
pointer, every emitted byte, output length and checksum for both recursive and
Array execution. They cover nested loops, empty/comment-only programs, skipped
nested loops, 255→0 and 0→255 wrapping, the left edge, growth through cell 17,
Hello World and 12 seeded bounded programs. A separate row runs the exact
`test.js` warmup/main strings and checksum `3562897308`, reparsing on each call.
The full native diagnostic additionally checks the full tape and output trace
against Rust; it is deliberately outside H's bounded test fixtures.

Test-only adaptations: recording output adds a List field and append at Print;
exposing the completed Tape replaces `Option[u32]` with `Option[Tape]` in the
extracted fixture. Array's initialized tape is reduced from 30,000 to 8 cells to
keep H within its unchanged step budget, and the growth vector exceeds that
bound. Production Array allocation and command behavior are unchanged. The full
native diagnostic adds two u64 counters; neither it nor output recording enters
the performance binaries. The Rust diagnostic adapter preserves the pinned
parser, Tape and recursive execute structure, adding output/call/visit recording
and exposing final state. It reads one program per stdin line.

Production adaptations retained from the original port: Hirsel List/u64 indices
replace Vec/usize; explicit u8 wrapping and a u32 print checksum replace Rust's
u8 methods/i64 accumulator. Modulo 2^32 print arithmetic and final checksums are
equivalent. Growth appends a single zero because each Next advances one cell;
Rust's resize(pos+1,0) is equivalent under that invariant. Hirsel's owned String /
Parser cursor and Option error shape remain unchanged. No new algorithmic
reference deviations are introduced by this optimization.

Reference identities verified at launch: recursive Rust source SHA-256
`4558418f83453acdfd9d43daf96e793890d2a5bd65bbe822989ccd1c735e6f2b`;
`test.js` SHA-256
`424371dd25f3fa3f779789c823a05dbe90d2eddbc2a3ca3c5e4319252c572797`.
MIT copyright/permission/warranty notice is retained in `bench/langarena/LICENSE`;
the Rust adapter cites the pinned file/range and notice.

## Reproduction and retained evidence

Build the two frozen packages from current-main baseline and candidate
`brainfuck.hir`, using the same `main.hir`, `harness.hir`, two-case registry,
stdlib manifest and file/clock/stdout grants. `hirsel build --release hirsel.json
-o langarena` builds each; main/harness source and runner argv are retained in
the evidence directory. Build an owned clone of upstream at the pinned commit
using `rustup run 1.99.0 cargo build --release --locked -j2`.

```sh
.buck2/bin/buck2 targets 'root//:'
.buck2/bin/buck2 test root//:hirsel-cli-test-la_brainfuck_recursion_10 -- --test-arg=cases::
python3 bench/langarena/la_brainfuck_recursion_10.py \
  --before /path/to/before/langarena --after /path/to/after/langarena \
  --reference /path/to/reference/rust/target/release/benchmarks \
  --reference-root /path/to/reference/rust \
  --toolchain-identities /path/to/toolchain-identities.json \
  --cpu 20 --rounds 10 --output /path/to/new-campaign
rustup run 1.99.0 rustc -O bench/langarena/la_brainfuck_recursion_10_reference.rs -o /tmp/bf-oracle
```

All actual consumed tracked/untracked sources, both package sources/manifests/
grants/configs, reference sources/Cargo.lock, compiler/runtime/toolchain and
executables are frozen with SHA-256 manifests and verified after their consumers.
The 30-process campaign manifest SHA-256 is
`9b475402a5d73ffb3f92198876653154b752f5950ebb33bfdde0bd5ed20dc711`.
Timing binaries before/after are respectively
`81c64cab889fdeedd475aeef3871e74151aee8a400c7367698f31414d1641a7b` /
`371a3c4bb443dc73e723e9a4474fcbbb38eb70a40a6524e36cf040337af93c30`.
Compiler SHA-256 is
`90dfbd42bf3380d27041c1634e769a128e4bf0f579a9b98adc937071f08715e9`.

Full sources/binaries, raw stdout/perf CSVs, profile sites, ten-round samples,
Rust vectors/full trace, disassemblies, compiler/runtime manifests and failure
logs are retained in `/workspace/code/hirsel-briefs/la-brainfuck-recursion-10/`.
`campaign/measurements.json` is the accepted three-participant instruction run;
`allocation-rounds/` is separate. Singleton/profile runs receive no speedup
credit. Historical `RESULTS.md` and the exact repository README are unchanged.

Failed/setup attempts are preserved individually: the first Rust build used a
PATH cargo wrapper whose remote output was not at target/release; the explicit
pinned Cargo retry produced the owned executable. Allocation attempt 1 had a
missing owned cwd and ran no program; attempt 2 omitted the std dependency and
failed to resolve wrapping functions; attempt 3 fixed both. A candidate setup
run used a relative tools/capped path from the frozen-package cwd and exited127;
its corrected singleton is setup-only. The first ownership Doc read found no
Doc board; the live plan at revision933 instead confirmed sole ownership of the
Brainfuck seam. The first hot-files check required main.rs for an immediate test-directory
registry; adding it exposed Cargo's duplicate implicit test name. Both setup
failures remain in separate logs. The coordinator identified this as a custom
standalone harness, so the final private cases/vectors move to tests/support/
with an explicit #[path] in the original root. Moved implementation/vector bytes
are verified unchanged; ordinary landing compiles the final registration.
No correctness assertion, production checksum, cap or safety
check was weakened to obtain a passing result.

The requested standard comparison command also completed independently:

```sh
HIRSEL=/path/to/frozen/hirsel python3 bench/langarena/compare.py \
  --upstream /tmp/ref-langarena --filter Brainfuck::Recursion --rounds 10 \
  --session la-brainfuck-recursion-10 --cpu 20 \
  --tools /path/to/owned/compare-tools \
  --output bench/langarena/RESULTS-la-brainfuck-recursion-10.md
```

CPU20 replaces reserved CPU31. This supplemental two-participant session uses the
unmodified standard runner and full LangArena registry, verifies source/toolchain
identities and all 20 checksums, and records PMU coverage and SMT observations.
It acquires its CPU20 advisory timing lock immediately, holding no other CPU lock.
All five counters have 100% running coverage. Its median instructions are Hirsel
30,142,544,507.5 and Rust 23,961,439,849.5 (H/R 1.25796), corroborating the scoped
three-participant campaign. Its standalone times are not before/after evidence.
The generated comparison report/projections are preserved as
`compare-generated-report.md` in the evidence directory; this reviewed report
supersedes it and makes no cross-host projection. Raw data are in
`compare-measurements.json` and the ignored `bench/langarena/build/la-brainfuck-recursion-10/`.
The supplemental source manifest is
`d311cd254ff54c558ad7a09f67ab633644fad64bb57ceb6549aca658276f481b`.

The only measurement follow-up is a locked before/after wall-clock comparison of
the same `run.js` recursion program; no semantic functionality remains open.
