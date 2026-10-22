# Native body-edit links — session 01a161f4

The fixture from incremental-profile-9 still rebuilds one of twelve objects in
Cranelift debug mode. Keeping DWARF embedded and reusing verified input digests
reduces median whole-process-tree retired instructions from 1,268,616,073 to
865,107,047 (31.81%). There is still one full ld.lld invocation per edit; this
change does not implement incremental linking.

## Matched controls

Baseline is pristine origin/main `45e18e8de4208f72e2a3ac147823a0e5530cf09b`,
built in its own detached worktree, never edited during a build or consumer.
Both compiler bundles and sibling std images, exact fixture, harness, environment
and every retained native toolchain/runtime input have SHA-256 inventories and
post-consumer verification. Root: `/workspace/code/hirsel-briefs/native-link-repeat-9/session-01a161f4`.

Ten seeded (`20261022`) interleaved rounds: before/after × debug/release, with
fresh independent namespaces per cell, one job, CPU6, daemon off, V on, ordinary
result/object reuse, no std objects or program shortcut cache. Each measured
body state is absent from its prepared stores. The helper changes `x + 1` to
`x + 2` and must print `6\n`. Debug is Cranelift O0 with DWARF; the user-facing
release control is LLVM optimized without DWARF. Release invalidates two
objects (optimization also depends on the helper), debug one; the control is
not a backend-isolated DWARF experiment.

`perf stat cycles,instructions` covers the compiler, threads and descendants;
all 60 measured samples have 100% running coverage. No exclusive wall timing
lock was held, and no wall ratios or speedup claims are made. Driver interval
shares below are attribution observations, not instruction shares.

| Cell | Instructions, median | Cycles, median | Link interval / total wall, observed median |
| --- | ---: | ---: | ---: |
| Before debug | 1,268,616,073 | 1,071,178,268 | 72.28% |
| After debug | 865,107,047 | 647,620,764 | 54.23% |
| Before release | 984,310,096 | 779,670,562 | 54.01% |
| After release | 850,206,964 | 647,909,823 | 42.59% |

Release instructions decrease 13.62%, independently of removing the debug
copy pair. Debug is initially 28.88% above release in instructions; afterwards
it is 1.75% above release, with the different backend/optimization work retained.

Each objcopy operation is measured separately for ten rounds using the same
fixture's candidate unsplit DWARF executable. Packaging that executable with
the original build-ID filename exactly reproduces both baseline executable and
baseline debug file bytes, so these are equivalent objcopy inputs.

| Operation | Instructions, median | Cycles, median |
| --- | ---: | ---: |
| --only-keep-debug | 107,727,998 | 105,973,152 |
| --strip-debug --add-gnu-debuglink | 144,542,786 | 127,346,759 |

The twenty objcopy measurements follow the forty interleaved compiler cells;
they are not added to those compiler totals. The strip operation depends on
its keep-debug predecessor. No optional linker or host dependency is added;
pinned ld.lld remains `--threads=1`.

## Attribution and allocations

Separate first-edit namespaces record descendant exec argv, instruction
samples at a fixed one-million-instruction period, and Callgrind, never a
previously cached body variant. Traces confirm one ld.lld and two llvm-objcopy
children before, one ld.lld and zero llvm-objcopy children after.

1,252 baseline and 861 candidate instruction samples: named linker processes
plus compiler stacks in `link_objects_impl`/`link_launcher` account for 683/1252
(54.55%) before and 297/861 (34.49%) after. The tool-only shares are 41.29% and
34.03%. These sampled named scopes are narrower than the driver's link interval;
output/toolchain work without those stacks stays residual. No stacks are double
counted. Raw mangled-symbol stacks and the attribution JSON are retained.

Allocation boundary calls, separate from PMU instruction evidence:

| Process | Before malloc / calloc / realloc | After malloc / calloc / realloc |
| --- | ---: | ---: |
| ld.lld | 19,334 / 123 / 2,551 | 19,330 / 123 / 2,551 |
| objcopy keep | 23,843 / 104 / 0 | absent |
| objcopy strip | 23,870 / 104 / 0 | absent |

Compiler mimalloc aligned allocation / zeroed / reallocation calls are
307,591 / 265 / 55,256 before and 307,597 / 265 / 55,267 after. Callgrind Ir
uses a different simulated CPU/loader and is not pooled with PMU counts.

## Correctness and cache contract

40/40 fresh/warm executable SHA-256 pairs match, each independently checked,
interpreted and executed with the expected output and no unexpected diagnostic.
Ten rounds also produce the same bytes across namespaces. Release is
byte-identical between compiler versions. Debug intentionally changes layout;
explicit packaging with the original split commands reproduces both old files
exactly, including the same build ID and DWARF. No language, IR, runtime ABI or
kiln change is involved.

Debug executables and debug runners keep DWARF embedded. Runner's public
`debug` path now names that executable; debug runners use a new layout cache
namespace so an old stripped runner cannot be reused as embedded DWARF.
GDB and LLDB separately list the remapped edited helper and actually stop at
its line-2 breakpoint; their commands, source copy, stdout and stderr are retained.
LLDB 18 was extracted privately for this check, with no system installation or
product dependency. Cross-backend/cross-target DWARF and source-root byte tests
are reported separately from those debugger checks.

Digest receipts are private, disposable and metadata-guarded, not content
identities themselves. Each entry binds absolute path, device, inode, mode,
size, nanosecond mtime and ctime. Newly changed/future timestamps are never
remembered (two-second racy-index window). Misses stream all bytes once,
accumulating both existing SHA-256 encodings, then verify opened and named file
metadata. Thus equal-size/equal-mtime changes, inode/device replacement and
symlink retargets miss without changing either build-ID or native-plan encodings.
Malformed or non-private receipts fall back to a complete read. Local coherent
ctime is the freshness assumption, as in Git; cross-user portable receipts are
not accepted. The ordinary result/object cache verification is unchanged.

## Prior art and deviations

Git `47fdf2208c5b94c8cc01c106b8f7c85efb91c926`, cloned under the evidence root:
`statinfo.c:64-105` supplies the stat comparison structure and
`Documentation/technical/racy-git.adoc:59-113` supplies the racy-entry full-read
rule. Adaptations: open/fstat the consumed file (follow input symlinks as the
linker does), compare device unconditionally on this Linux native path, use a
conservative two-second exclusion instead of Git index-file time, and store
both existing digest encodings for byte compatibility. No Git implementation
is vendored. SHA-256 remains the existing sha2 implementation.

Cargo reference in rustc-perf `10c88d40d41a89346b106eeb9c9b02ddc69c735b`,
`collector/compile-benchmarks/cargo-0.87.1/src/cargo/core/profiles.rs:289-301`,
avoids a packaging pass using unpacked debug information on Apple targets.
That source does not establish a Linux default of unpacked DWARF. Our Linux
choice is embedded DWARF, which GDB and LLDB consume directly. It avoids adding
a new debugger object-discovery contract or packaging flag.

## Verification and attempts

The original baseline build overlapped source editing and is excluded as
`mixed-source-hirsel-uncertified`; its log is retained. The accepted baseline
was rebuilt from a separate pristine checkout and its produced SHA-256 is
`f646520324520fffea227a44ec2b91bc5f5f197404e3fb00066d145dfeb50856`.
The measured candidate SHA-256 is
`61ccbc9b1bf69901d583cdda1c6f9227dcd91af152452347e96dd14588420efb`.
The candidate source files were frozen through its build, matrix and profiles;
complete hash-verified source archives survive worktree cleanup. The measured
candidate's only later production-code change is extracting its existing
post-read metadata comparison and enriching the refusal diagnostic with the
opened/named states, plus a deterministic replacement-during-read regression.
That does not change the successful digest or link path. No accepted matrix
or profiles were repeated for that diagnostic/test change or moving main.

One initial Buck invocation used unsupported executor filter syntax and ran
zero inner tests; it is setup-only, with its failure retained. Correct syntax
is `-- --test-arg=<filter>`. The first one-round harness smoke incorrectly
expected the release cell to have one object miss; the actual optimized
control has two. It was excluded, the assertion was corrected specifically
for release, and the accepted ten-round matrix still requires debug's one
object miss. A corrected one-round smoke precedes the final matrix.

Initial scoped CLI DWARF selection: three tests passed, one failed closed
because a runtime archive's metadata changed during hashing. The initial
log lacks the stamp values, so no claim is made about whether bytes changed
or an input was rematerialized. An enriched-diagnostic rerun of only the
failed test passes (1/1); all freshness checks remain intact. A new deterministic
replacement-during-read test checks refusal when the opened descriptor and
named input diverge. The three already-green CLI tests were not repeated.
GDB and LLDB probes first used an incorrect standalone source prefix; those
setup logs are retained, and the corrected package prefix provides actual
source listings and stopped breakpoint evidence.

The integration selection passes 2/2: concurrent deterministic links with and
without debug on both targets, and the debug runner's identity, embedded file
and native execution. CLI DWARF covers 4/4 identities after the targeted rerun:
both backends on both targets, generated body source, source-root byte parity,
and cached source insertion/file movement versus fresh emission.

Reproduce using retained bundles and toolchain; use a new work directory and
run outside compiler builds. Long commands retain their EXIT marker:

```sh
S=/workspace/code/hirsel-briefs/native-link-repeat-9/session-01a161f4
P=/workspace/code/hirsel-briefs/incremental-profile-9/session-01a161c9
(tools/capped --timeout 900 taskset -c 6 tools/compiler-bench/native-link-repeat-9 matrix \
  --before "$S/before/hirsel" --after "$S/after/hirsel" \
  --env "$P/frozen-environment.json" --work "$NEW_WORK" --rounds 10 \
  --seed 20261022 --freeze "$P/native-inputs" > /tmp/native-link-repeat-9-matrix.log 2>&1
 echo EXIT $? >> /tmp/native-link-repeat-9-matrix.log)
# `profile` uses another new WORK for per-process traces, samples and Callgrind.
.buck2/bin/buck2 test //:hirsel-driver-unittest -- \
  --test-arg=native::file_digest::tests --test-arg=native::kit::tests \
  --test-arg=native::runtime_cache::tests
.buck2/bin/buck2 test //:hirsel-cli-test-cli-shard-1 -- --test-arg=dwarf::
.buck2/bin/buck2 test //:hirsel-driver-test-it-shard-1 -- \
  --test-arg=link_kit::shipped_links --test-arg=runner::a_debug_runner
```

The retained per-session script and the landed script differ only by removal
of an unused import. All source certificates, PMU CSVs, perf.data, per-PID
Callgrind files, source edits, output hashes, commands and filtered environments
are retained. Objcopy counts include the tiny shell cd/exec dispatcher; both
are separate from compiler totals. Wall-clock speed ratios and optional
parallel-linker performance remain follow-ups; no CPU lock was acquired.

Final unit selection passes 15/15 (six new digest tests, two link-kit tests,
seven runtime-cache tests); the crate's test inventory grows 98 → 104.
The extra CLI consumer of the DWARF helper removes the embedded executable
once in its cleanup, rather than treating it as a distinct sidecar.

Measurements consume copied sealed bundles, never buck-out executables. Focused
Buck test-harness compilation on the pool overlapped some measurements without
modifying those bundles; this is another reason to retain PMU instruction
counts as the verdict and make no wall-clock comparison.

The standalone cross-target/qemu and debug-line CLI consumer also passes 1/1.
Total distinct focused Rust test identities green: 22 (15 unit, 5 CLI, 2
integration), with the initially failed DWARF identity's refusal and targeted
rerun kept in the record. Existing test inventory before/after for that final
unit selection: 9 → 15; six new cases. No baseline test execution count is
inferred from an inventory or from a failed executor setup.
