# Native remaining attribution — session 01a16201

This is attribution, with no compiler optimization or speedup claim. The original
39.93 percentage-point residual from `incremental-profile-9` remains the absolute
reference denominator. A new current-source diagnostic does not replace that
historical denominator.

Evidence is retained at
`/workspace/code/hirsel-briefs/native-remaining-attribution-9/session-01a16201`.
The predecessor `session-01a161c9` is unchanged. Current frozen compiler/runtime/std
source is `d5b594d8250ad3c306e3caca6b68aa7cb5b754ba`; native tool inputs are private
copies of the predecessor's pinned LLVM/link-kit/Valgrind, with current runtime
archives. `source.tar`, `bundle`, `native-inputs`, `frozen-environment.json`, and
per-consumer `inputs.json` retain SHA-256 identities. New result/object/temp/cache
namespaces never reuse the predecessor's caches.

The minimal trigger is the existing `tools/tests/fixtures/incremental-profile-9`:
`helper(x:u64)->u64{x + 1}` becomes `x + 2`, and main calls `helper(4)` and prints
`6\n`. Signature is unchanged. Unchanged prints `5\n`; the signature control changes
the helper parameter/result to u32. Each cell warms the original source first,
records its patch, applies exactly one edit, and restores exact original bytes.
Every edited source passes an independent cache-free check, interpreter execution,
native build and execution. Diagnostic output is checked, not discarded.

## Controlled instruction evidence

Ten rounds, seeded order `20261022`, three native edits × telemetry off/on =
**60 measured cells**, **60/60 native output pairs**. Ordinary CPU6, one compiler
job, 4 GiB cap, no CPU31 use, compiler threads and descendants inherited by
`perf stat cycles,instructions`. CPU7 timing lock was busy; it was tested once
without waiting or holding another lock. All wall times remain observations;
no wall ratios. Other lanes and SMT can affect cycles and wall time.

| Edit | Plain instructions | Telemetry instructions | Full V | Lowerings | Object hit/miss | Value decode/rebind | Encoded Value bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Unchanged | 1,275,413,266 | 1,275,003,194 | 0 | 0 | 12/0 | 12/12 | 67,171 |
| Body | 1,273,166,300 | 1,270,985,608 | 1 | 1 | 11/1 | 11/11 | 62,946 |
| Signature | 1,313,577,656 | 1,315,729,232 | 2 | 2 | 10/2 | 10/10 | 30,275 |

The telemetry/plain median differences are −0.032%, −0.171%, +0.164% respectively;
these bound this session's observed counter/reporting effect and noise, not a
claim that telemetry improves performance. Counts are constant over each group.
Effect replays equal Value rebinds. The byte counter is encoded Value text
length (`unit.len()`), excluding the outer JSON envelope, layouts/ABIs and embedded
effect/type-table byte array. It is not total store I/O; a future opt-in
`bytes.len()` counter immediately after `Imports::get` store lookup is the exact
remaining envelope-byte seam. Native `units` counts actual Ready lowerings
(0/1/2), rather than all 12 admitted Values. Existing counters suffice; no Rust
instrumentation or cache/validation policy changed.

## Original residual, disjoint partition

The analyzer reads demangled `perf script` output with explicit periods. Each
sample enters exactly one bucket, weighted by its recorded instruction period.
External processes first, then the predecessor's SHA/from_image/varint **leaf**
exclusions preserve its original partition. Resource leaves (kernel, allocator,
loader) remain separate from their semantic callers. Digest, view checking,
receipt reconstruction, full body V, import and backend are separate decisions.
A SHA leaf nested under V is excluded once; it is never added as another share.

The original 235 samples represent 1,268,118,516 sampled instructions, zero lost;
Kish effective sample count is 203.57. Original residual weights sum to 39.9328%
from full-precision periods; scaling only those weights to the published 39.93
preserves its rounded denominator exactly. The following are percentage points
of the **original whole tree**, not percentages of just the residual.

| Exclusive residual cause | Original denominator points | Confidence / minimal route |
| --- | ---: | --- |
| Kernel | 15.216 | Medium process/leaf evidence; page clearing, anonymous faults, VMA/memcg accounting; causal split remains open |
| Additional std-image work | 4.653 | Medium; stack reaches StdImage/image rather than a generic decode leaf |
| Allocator leaves | 3.386 | Medium; allocator boundary/leaf cost, not caller-inclusive allocations |
| Native cached import key/decode/rebind work | 2.244 | Medium; import_cache/rebound/effects stacks, distinct from std bincode |
| Validation input digest, excluding already assigned SHA leaves | 2.124 | Medium; exact input::digest stack |
| Dynamic loader | 1.724 | Medium; ld-linux/_dl leaf, compiler/executable startup |
| Actual signature-view checks | 1.313 | Medium; check_view/signature_valid, not full body V |
| Full body V checking | 0.615 | Low sample precision; one counted full V |
| Backend | 0.544 | Low; one native object miss |
| Link identity remainder | 0.411 | Low; route to native-link-repeat-9 |
| Unresolved symbol | 0.337 | Explicit irreducible unknown |
| Builtin registry startup | 2.647 | Medium; registry::build stacks, independent of std image decode |
| Native operation manifest | 2.577 | Medium; adapt::manifest/classify and its callers |
| Native snapshot export/import | 1.671 | Medium; export_native_paths/import_native_against |
| Front mode-cache decode | 0.468 | Low; ModeRecord decode stack |
| **Total residual** | **39.930** | Rounding of displayed rows only |

The other original buckets stay 41.69% external ld.lld/objcopy, 13.61% SHA leaves,
and 4.77% std/from_image + generic varint leaves. Their nested SHA link/V subshares
are annotations, never additions. `predecessor-partition-final.json` retains exact
weights, per-process totals, descriptive Wilson intervals using effective n, and
top leaves. Those intervals express sampling precision, not independent-run
confidence or systematic unwind/symbol bias. The unresolved-symbol bucket is 0.337 whole-tree points (0.845% of the original
residual). Zero original samples remain in other-unassigned after applying the
recorded caller rules; that is not evidence that startup is fully understood.
Kernel work is identified, but its exact split between mappings, faults and I/O
remains unresolved. The denser current profile also retains unknown work.

## Receipt versus actual checking

The retained predecessor Callgrind capture has 523 incoming calls each to
`hirsel_validate::input::digest`, `ValidatedSignatures::verdict_from`, and
`input::check_view`, versus exactly **one** `hirsel_validate::validate` full body
check and **11** `objects::rebound<Value>` calls. `emission::check_receipt` runs
once for the rebuilt Ready object. These signature checks are actual work;
calling all verify-phase time full body V or receipt replay is incorrect.
`verdict_from` can return a retained verdict after checking its reads, or call
`check_view`; source `crates/hirsel-validate/src/input.rs:264` is the boundary.
`queries.rs:1989` is the full-V counter boundary. Cached native Values go through
`native/import_cache.rs:253` and `hirsel-rc/src/import/mod.rs:384`; those are
separate from std-image loading and TIR validation.

`callgrind-functions.py` retains exact self-Ir conservation per process and
incoming entry counts. Incoming inclusive costs are cross-cutting diagnostics,
never summed. Callgrind runs a different CPU/loader model and excludes kernel
instructions; simulated Ir is never pooled into PMU shares. Allocation boundary
counts are counts, not bytes, distinct objects, live memory, or total heap
churn. Heaptrack's missing mimalloc coverage cannot prove compiler totals.

## Resident observer limit

`crates/hirsel-cli/src/serve.rs:134` refuses resident dispatch for timing/memory
flags; `main.rs:930` dispatches before `measure.start()`, and `measure.rs:61`
enables global timing collection only in the fresh process. The existing owned
server PMU enable/ack protocol measures whole resident instructions, but exports
no request-scoped engine phase/query timing report. The exact missing seam is a
request observer around the resident worker's execution plus a response field;
no public dispatch or daemon change is made. Accepted predecessor resident PMU
controls are reused; no new resident campaign is needed.

## Reproduction and references

Build and freeze the scoped CLI/std-image/runtime targets **before** profiling.
Discover with `.buck2/bin/buck2 targets 'root//:'`; use
`tools/compiler-bench/native-remaining-attribution-9 matrix|profile` with
`--cli FROZEN_HIRSEL --env FROZEN_ENV_JSON --work NEW_SESSION --rounds 10 --cpu 6`
and freeze `bundle`, `native-inputs`, `source.tar`. `matrix` scopes the predecessor
harness to native unchanged/body/signature plus telemetry controls. `profile`
independently warms/restores a namespace for each of DWARF instruction sampling,
per-descendant Callgrind, and process tracing. Never profile an already-cached
body variant. All profiler cells assert one full V and one object miss.

Convert a retained perf file with `perf script -F
comm,pid,tid,period,event,ip,sym,dso` and demangle with `rustfilt`. Analyze with
`native-remaining-attribution-9 analyze SCRIPT --lost-samples 0
--script-sha256 SHA256 [--historical] [--instructions WHOLE_TREE_TOTAL]`.
The script rejects malformed/empty stacks, changed input hash, lost samples and
missing distinct descendants. `--objcopies 0` explicitly describes an embedded
DWARF topology; the historical two-objcopy topology is the default. Estimated
instruction totals scale sampled shares; they are not independent per-PID PMU
measurements. `validate_session` additionally rejects changed frozen inputs,
failed/missing body cells, incomplete PMU coverage and wrong full-V/object counts.

Rustc dev guide pin `aa181c58c5ad008b67fa8df22b3e610cb9c81dc8`,
`src/profiling/with-perf.md:18–48` supplies DWARF/build-before-profile procedure;
`src/queries/incremental-compilation-in-detail.md:16–105` distinguishes changed
inputs, recomputation and changed results. Full-file SHA-256 identities match
`BACKLOG-10-REFERENCES.json` and are retained in `reference-identities.json`.
MIT/Apache-2.0 notices remain in the predecessor's references directory. Existing
harness controls at `02e6b07abf7881a5e4fe2477f32940a9fcd44150`,
`tools/tests/incremental_profile_9.py:19–65`, also match their pinned full hash.
LangArena retained source is MIT, Copyright 2026 Konstantin Makarchev.

Hirsel adaptations: preserve incremental caches; execute the helper edit with
output oracle; count actual V separately from signature and receipt work; retain
kernel and unknown costs; use fixed instruction periods for a short bounded
higher-density profile; scope new rounds to three native controls rather than
repeat the accepted resident/disk campaign. No external implementation is
vendored, and no language/IR/runtime ABI change is made.

## Higher-density current diagnostic

The first capture (`profile-1`) recorded 4,256 samples but reported **25 lost
chunks**. It is excluded from the sampling partition. Its symbol reader was
interrupted after loss was noticed; the raw capture and failure disposition are
retained. Independently restored Callgrind and process-trace cells completed
with one full V, one object miss, and correct reused/fresh output `6`.

`profile-2-stacks` changes only the sampling setup: ordinary CPU5, fixed period
500,003 instructions, 8 KiB DWARF stack, 1,024 mmap pages, and `perf script
--no-inline`. It collected **2,431 samples, zero lost** in a new warmed namespace.
The 1,215,507,293 period sum has this process partition: initial CLI `hirsel`
60,000,360; compiler thread `hirsel-compiler` 646,003,876; ld.lld 290,001,740;
two objcopies 219,501,317. The compiler/launcher union is 58.08%, tools 41.92%.
There are four distinct PIDs; compiler threads retain the same process PID.

The separate ten-round timed-body PMU median is 1,270,985,608 instructions.
The sampling period sum is 4.36% lower; these are separate diagnostic and matrix
acquisitions, with sampling tails/skid and profiler effects. Estimated bucket
instructions in `current-partition-final.json` scale the sampled shares to that
whole-tree median and sum to it exactly. They are **estimates**, not per-process
PMU counts or a measured improvement. No current fraction is subtracted from the
original 39.93-point denominator.

| Current exclusive bucket | Sample share % |
| --- | ---: |
| External linker | 23.858 |
| External debug-copy processes | 18.058 |
| Kernel | 15.097 |
| Std image | 9.132 |
| Link identity | 9.009 |
| Builtin registry startup | 4.895 |
| Allocator leaves | 3.949 |
| Validation input digest, including its SHA | 3.661 |
| Native manifest | 2.057 |
| Cached native import | 1.892 |
| Signature view checking | 1.563 |
| Loader | 1.275 |
| Native import planning | 0.864 |
| Native snapshot | 0.740 |
| Backend | 0.370 |
| Front cache decode | 0.329 |
| Receipt reconstruction | 0.288 |
| Link output | 0.206 |
| Memory primitives | 0.165 |
| Full body V | 0.123 |
| Front checking/modes/lowering | 0.041 |
| Native LIR validation | 0.041 |
| Other unknown | 2.221 |
| Unsymbolized | 0.165 |

Kernel top leaves include page-fault entry, page clearing and `_copy_to_iter`.
Those are retained as kernel, not misattributed to parse/V or folded into user
allocator work. Current receipt reconstruction has only seven samples, 0.288%
(descriptive 95% precision interval 0.140–0.593%); signature-view checking is
1.563% and full body V 0.123%. The latter is small enough that counter evidence
is stronger than its sample fraction. Current unknowns include SHA with no
recognized identity caller and generic cache/type decode; they remain explicit.

The current per-process Callgrind self-Ir totals are 2,951,466,705 compiler;
224,728,981 linker; 66,622,148 first objcopy; 86,802,459 second objcopy. All self
rows sum exactly to each process summary, with no nested inclusive double count.
Current entry counts reproduce **523 digest / 523 verdict / 523 signature checks,
1 full V / 11 Value rebounds / 1 Ready emission receipt check**. Compiler mimalloc
boundaries are **307,591 malloc-aligned / 265 zalloc-aligned / 55,256
realloc-aligned**, equal to the predecessor. libc malloc/calloc/realloc boundaries:
linker **19,341/123/2,551**, objcopies **23,744/104/0** and **23,771/104/0**.
These are function-entry observations under simulation, with no allocation-byte
or total-live-memory interpretation.

## Retained-suite marker, separate diagnostic

The copied frozen predecessor registry has **47** names, confirmed from
`bench/langarena/build/registry.hir`. The exact marker patch changes only
`checksum mismatch (RECOMPILE_MARKER_0)` to `_1` in `main.hir:53`; no new suite
or runtime campaign runs, and there is no Rust comparison.

First attempt `suite-marker-1` correctly failed frozen-input verification:
ordinary dependency loading found the current std image did not match the
retained old suite's package key. `crates/hirsel-engine/src/package.rs:736–747`
calls `compiler.image_package` and atomically replaces the host-selected file;
`image.rs:398` selects the sibling `hirsel-std.image`. The changed bytes were
preserved separately, the original current-source image restored from the owned
Buck output, and earlier accepted manifests reverified. This later mutation does
not invalidate consumers that had already completed and verified their inputs.
No accepted matrix, lossless profile or Callgrind was repeated.

Failed-only retry `suite-marker-2` uses a separately copied compiler/image pair,
certified before preparation and frozen through completion. `compiler-image-pairs.json`
records the pairs: same compiler `90dfbd42bf3380d27041c1634e769a128e4bf0f579a9b98adc937071f08715e9`,
one-body image `3d78c7cfbb83dff12a0d25301236457b4e5a3914ab754cb9918f56a26e4da9a7`,
retained-suite image `7e40a616053299b371690d7b2d259d7ebacfdbccc20a4175acd08cd24c2d3c7d`.
The owned one-body bundle remains restored. Both failed and corrected suite
attempts and their logs survive. The first attempt's numbers are not accepted
performance evidence.

The corrected marker passes post-consumer hashes and reproduces historical
**54 full Vs / 9 TIR lowerings / 829 object hits / 1 miss**, plus **745 native
Value decodes/rebinds/effect replays, 32,762,432 encoded Value bytes, 31 Ready
unit lowerings**. One object miss can require Ready copies of other bodies; it
does not imply exactly one Value import or Ready traversal. Its native import
phase reports 2.253 s of work (diagnostic, not a benchmark ratio).

The suite capture uses CPU5, one job, release Cranelift, instruction period
2,000,003, 16 KiB DWARF stack, 1,024 mmap pages: **20,469 samples, zero lost**,
40,938,061,407 sampled periods. The outer perf-stat scope also includes the
profiler; its 44,423,115,872 instructions and 100% PMU running coverage are
retained **without treating that as compiler-only cost or a performance ratio**.
No total from this suite is mixed with the one-body denominator.

| Suite exclusive cause | Sample share % |
| --- | ---: |
| Cached native import key/decode/reconstruction | 41.844 |
| Allocator leaves | 8.232 |
| Native import planning | 6.717 |
| Kernel | 3.899 |
| Full body V checking | 2.682 |
| Memory primitives | 2.648 |
| Front cache decode | 2.619 |
| Std image | 2.013 |
| Native snapshot | 1.803 |
| Front checking/modes/lowering | 1.480 |
| Link output | 1.304 |
| Receipt reconstruction | 1.221 |
| Validation input digest | 1.109 |
| External linker | 0.728 |
| Native LIR validation | 0.401 |
| Link identity | 0.332 |
| Backend | 0.249 |
| Builtin registry startup | 0.142 |
| Native manifest | 0.068 |
| Loader | 0.039 |
| Signature view checking | 0.034 |
| Other unknown | 18.897 |
| Unsymbolized | 1.539 |

The importer is dominated by JSON byte-array/number parsing and reconstructing
cached Values/effects, plus recomputing canonical import keys. It is not repeated
TIR checking. Source `import_cache.rs:253` decodes the JSON envelope, embedded
type/effect table and text Value; `get:279` computes the key before the store
lookup. `hirsel-rc/src/import/mod.rs:384` rebinds the Value after replaying builder
effects. This path does not invoke full TIR V. Of the 41.844 whole-suite points,
24.085 have decode/type-reconstruction stacks, 12.585 key/encoding stacks,
0.015 direct rebound stacks, and 5.159 other cache stacks; these are **nested
annotations**, not added again. Resource leaves remain outside this cache bucket.

Signature/receipt validation and the 54 actual body Vs still occur, but their
separate samples do not explain the 41.844% cache path. The suite's explicit
20.436% unknown bucket remains substantial: generic bincode byte decode, SHA
without a recovered identity caller, codec formatting and truncated stacks.
Do not assign it all to receipts, V or import. The sampling capture and
`partition.json` retain those unknowns, rather than forcing the minimal
fixture's partition onto the suite.

## Implementation-ready routes and open limits

All follow-ons below retain V and content verification. None is implemented by
this attribution change.

| Owner / route | Minimal reproduction | Next concrete change or diagnostic |
| --- | --- | --- |
| app-perf-refresh-2 | Fresh CLI on unchanged/body fixture with no std selected | Separate lazy std-image loading, builtin registry creation and initial ELF mappings; preserve selected std semantics and compare fresh/resident independently |
| native-link-repeat-9 | Same helper body, debug build, retained process.trace | Kernel file copy/read/fault split using process-scoped faults/syscall stacks; keep CPU/system counters separate. Link/hash/debug-copy implementation already landed as `3852e758`; this report's d5b594d8 fractions predate it and are not additive to its gains |
| mc, with app-perf-refresh-2 | Same one-job fixture, unchanged versus body | Attribute mimalloc boundary callers/held objects; preserve allocation counts separately from bytes and PMU; no multicore scaling inference |
| verify-cache-modes-9 follow-up via @agentlang-24 (lane now settled) | Same body edit; 523 view digests/checks versus one full V | Trace `ValidatedSignatures::verdict_from` candidate/held-read outcomes and `check_view` calls, plus image-key auto-refresh. Preserve policy and independently validate any later optimization |
| @agentlang-24, native import follow-up | Retained 47-task suite marker only; 745 decoded Values / 32.76 MB | Split `import_cache::get` canonical input serialization from envelope/type/effect/Value decode and builder replay. Optimize a separately measured seam only after exact one-miss/output/diagnostic parity |
| @agentlang-24, native manifest/snapshot follow-up | Std-free helper body; adapt::manifest and export/import native paths | Measure operation classification and registry/signature-map reconstruction directly before changing indexing/retention; original residual points 2.577 + 1.671 |
| @agentlang-24, residual symbol coverage | Same lossless captures, no new compiler campaign | Resolve original 0.337-point symbol gap and current unknowns (2.386% minimal / 20.436% suite) with better stack/source identities or targeted existing observers; leave unresolved work explicit |
| @agentlang-24, resident observer | Existing resident check with owned enable/ack PMU | Request-scoped internal timing/query observer; public timings currently bypass resident dispatch. Architecture/dispatch redesign remains out of scope |

The original kernel bucket remains a defensible kernel cause with unresolved
subcauses, not proof that an allocator, parser or V owns it. No further wall
campaign, validation bypass, V digest optimization, cache redesign or language
change is part of this result.

## Verification and attempts

Before: existing incremental harness **7 passing unit controls**, new analyzer
coverage absent. After: **8 analyzer tests + 7 unchanged predecessor tests = 15
passing**, through the two exact scoped Buck targets. The final 8-test run
checks malformed/empty stacks, weighted conservation, nested-share exclusion,
receipt/full-V separation, lost captures, changed inputs, failed/zero/missing
body samples, incomplete PMU and missing distinct descendants. No broad corpus
or workspace test campaign was run. The new Python harness has its own explicit
`tools/buck2/sync.py` registration; generated BUCK is regenerated, never committed.
No ordinary Rust module registration is needed.

Retained failed attempts: initial empty-parser error (6 tests, 1 error); two Buck
launcher permission failures before the executable script/cache key changed;
intentional bad-hash analyzer negative control; an incorrect rustfilt file-argument
invocation (rejected malformed profile, corrected to stdin without recapture);
first sampling loss; suite image mutation and the consequent retained-manifest
check before image restoration. Each has a separate file/log; none is relabeled
a green sample. Final matrix is 60/60, native binary-byte comparisons 60/60,
lossless minimal sampling 2,431, corrected suite sampling 20,469. All active
captures ran serially under the unchanged 4 GiB cap; matrix peak 3,751 MiB,
first profile 794 MiB, corrected stacks 162 MiB, failed suite setup 3,158 MiB,
corrected suite setup 1,014 MiB. No cap refusal occurred and no cap was raised.

The matrix/profile-v1 consumer harness is retained as `consumed-harness-v1.py`;
final sampling validation/rules and capture options changed only after those
consumers completed and verified. `retained-inputs.json` maps removed checkout
paths to exact consumed copies, including that earlier harness; it does not
replace the original manifests. The matrix method/compiler/fixture/native graph
is unchanged, so accepted rounds are reused without running them again.
