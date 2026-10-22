# Checked initialized-byte replacement

The additive `core@1/bytes.replace_range` primitive implements ordered initialized-range checks, snapshot-preserving COW, unchanged length/capacity, valid-empty no-op behavior and exact shared-arena charging. The HTTP candidate is rejected: it materially regresses persistent small replies despite a substantial mixed large-reply improvement. The landed HTTP serializer retains its original scalar loop. The ≤3× Rust persistent static GET product target remains open until same-session evidence supports it; Cachegrind cannot establish native latency or throughput.

## Inputs and scope

Launch baseline: `ab44bbb731be91d79289eb3df88aeb139e3dfb75`. The implementation and unchanged production baseline use separate owned worktrees. Original eleven-case sequence sources/twins and physical clients were copied from `/workspace/tmp/http1-output-cost-4`, never rewritten. The scripted source/twin SHA-256 values match the design freeze: `35735548a9ce865b89d67b505872584ee64462a6171e606e3a2277228ff8cc49` and `240ec1b7239509acb959bc0970afcbf8f89e3a48a2ecebbd47b054d4d0f0e81a`. Neither rejected historical patch was restored.

Evidence directory: `/workspace/code/hirsel-briefs/http-replace-range-impl-7` (B). Reproduction artifacts: `/workspace/tmp/http-replace-range-impl-7` (O). `input-freeze.json`, `toolchain-freeze.json` and `consumed-freeze.json` seal source, stdlib, toolchains, clients/configuration, executable identities and compilation commands/statuses. Complete repository input manifests separately seal 11042 source/build/configuration files during validation. Before/after use the same candidate compiler SHA-256 `6b95643f272ccabfc0bb6b94b626e0d18acb2ea294aca62d99d8ff5e46acce47` and runtime. Their stdlib snapshots differ only in `http1_parse.hir`. Current-production compiler SHA-256 is `8a53c69ebdf3a676a82f35916e942fc80986e87c89b864e4dbbbcd2f3c9b2506`.

The API has `@preserves_unique(builder)` and `@unretained(data)` metadata and ordinary MayAllocate effects. Invalid destination intervals raise MEM-14 after argument evaluation: first `start <= L`, then `N <= L-start`. Invalid source construction still raises SEM-3 first. Valid empty intervals bypass backing preparation. Nonempty writes use existing shallow COW after validation; bounded shared storage charges L logical bytes once, never N or spare capacity. The physical leaf repeats checks, uses overlap-safe copying and publishes no length. H updates the prepared allocation instead of generic append/update allocation. Normal failure, cleanup shielding and mutable completion apply.

Language rules and TIR/LIR constructs/schema are unchanged. Runtime ABI revision 38→39 adds `hirsel_rt_v2_bytes_replace_range(TaskCtx*, Header**, u64 start, Header* source_owner, u64 origin, u64 length) -> u8`; existing fields/layouts/signatures are preserved. No backend-specific instruction or new error arm was added. The generated core review and native operation identities come from `tools/bless.sh core-review native-inventory`, followed by successful no-bless checks; no generated build inventory or API page is committed.

## Validation

The new modules had zero tests before this change. Final new-test total: 5 driver + 2 RC + 3 runtime = 10 passing tests, split across the actual logged selections below, without rerunning already green primitive checks.

| Evidence log in B | Actual result |
| --- | --- |
| `primitive-2.log` | 2 runtime tests pass; driver/pipeline CAS input-identity failure receives no semantic credit |
| `primitive-3.log` | 1 RC noalloc/effect test passes; other setup issues retained |
| `rc-4.log` | 1 RC control-flow test passes |
| `driver-5.log` | 4 driver tests pass; cleanup fixture fails during source checking |
| `cleanup-7.log` | corrected cleanup test passes (1); six MEM-14/SEM-3 cleanup cases and exhausted-arena shielding |
| `arena-leaf.log` | added same-owner prepared-arena ABI test passes (1) |
| `identities.log` | generated core-review and native inventory checks pass |
| `http-controls-2.log` | 24 affected HTTP/1 tests pass, 441 filtered |
| `http-boundary.log` | unchanged retained boundary/alias test passes (1) |
| `http-short-writes.log` | candidate partial-write test passes (1), separately retained as HTTP evidence |
| `http-native-selection.log` | selected corpus 10/10 and Cranelift 10/10 pass; combined native smoke action times out after 600 seconds with no case output |
| `abi-header.log` | C header/runtime declaration agreement passes (1) |
| `native-smoke-inline-errors.log` | isolated inline-errors case passes: 4 configurations, real Io record/replay, Cranelift and runner oracle; 11 image runs, no executable fallback |
| `descriptors.log` | all descriptor/export/locality audits pass (12) |

Primitive driver parity compares ordinary, checked and record/replay observations; H includes fault injection; native controls include LLVM O0 diagnostic, LLVM O3 production and Cranelift diagnostic. Native observation projection checks output/failure cause/sites; cleanup probes assert one finalizer and one close message. The RC test follows actual normal CFG paths and verifies ordered checks plus the empty branch before IsWritable/UniqueCopy, without ArenaPush, SeqReserve or SeqSetLen. Direct and function-valued calls share that wrapper. Runtime prepared-storage checks retain capacity 128, initialized length 5 and spare sentinels, including overlap and u64-limit arithmetic without giant allocation.

The unchanged HTTP boundary fixture covers 0/3/65535/65536/65537 then smaller HEAD/204/304 replies and aliases across interpreters/replay/H/LLVM/Cranelift. Existing invalid-response controls retain zero write/write_all, nine closes and zero shutdown, with status/header/reserved-header precedence. The owned partial-write adapter accepts 1,2,remainder, compares exact concatenated wire, retains the source and tests failure before/after partial output, timeout setup, >65536 splitting and exactly-once close. It uses existing transport semantics; no public transport change was made.

The three proposed fixture/twin pairs were omitted: Rust-generated vectors avoid the 1M corpus budget, and the existing scripted write_all schema cannot encode partial counts. An owned adapter in the private Rust module supplies that control. No corpus rows or shared fixture bodies were changed.

A source-level same-owner arena Bytes view is inaccessible under existing PROV-4: `arena.bounded(9, fn(a: Arena)->u64 { let b: List[u8] in a = [1u8]; let src = bytes.from_list(copy b); 0 })` is rejected because from_list requires heap-owned input. Existing provenance/from_list rules were retained. Source tests use retained List aliases and an independent static source; the same-owner physical arena ABI case is covered separately by runtime prepared-storage tests, with exactly 5 bytes of COW allowance succeeding and 4 failing before copy.

All setup/fixture failures are preserved: initial wrong target selection executed zero tests; missing Callee import; own input edits during primitive-2 caused CAS upload identity mismatch; entry/body CFG assumption and RuleId clone compile error; reserved identifier and noncopyable Test fixture mistakes; missing generated native identity; cleanup-6 PROV-4 rejection; http-controls initial forwarding of --skip executed zero tests. Corrected retries run only unproved checks on sealed inputs. The historical combined timeout and this lane's native timeout are not semantic passes or transport failures. The combined native smoke action digest is `46081a4ea28c82a9d39f408380e76524619e417042663da112845c5a612a1a26:147`. It emitted no case/stage output. An isolated first selected case completed in 134.134 seconds within the same 600-second limit; this establishes a case-specific success, not proof that all remaining stages of the combined action finished. The other nine HTTP smoke selections remain unexecuted because the candidate serializer was rejected; corpus and Cranelift results remain credited separately. No budgets were raised.

The production baseline did not execute a fresh 24-test HTTP selection; the earlier historical 24-pass report is historical evidence only. This lane's 24-pass selection used the frozen HTTP candidate. No already-green primitive test was rerun after restoring the scalar serializer: all five new driver tests, including the partial-write control, had already passed with that original serializer before candidate integration. The revert changes no primitive/runtime/lowering input. Final fmt, hot-file and diff checks run on the primitive-only tree; tools/land supplies required integration checks.

Scoped test command forms (each original selection is preserved by its named log; these are reproducible groups, not additional executions):

```sh
.buck2/bin/buck2 test //:hirsel-rt-test-it -- --test-arg=bytes_replace_range
.buck2/bin/buck2 test //:hirsel-pipeline-tests-test-rc-shard-1 -- --test-arg=rc_bytes_replace_range
.buck2/bin/buck2 test //:hirsel-driver-test-it-stdlib-shard-1 -- --test-arg=http_replace_range
.buck2/bin/buck2 test //:hirsel-driver-test-it-stdlib-shard-1 -- --test-arg=stdlib_http --test-arg=--skip --test-arg=http2 --test-arg=--skip --test-arg=inline_output_boundary_aliases_and_replay
.buck2/bin/buck2 test //:hirsel-driver-test-it-stdlib-shard-1 -- --test-arg=inline_output_boundary_aliases_and_replay
.buck2/bin/buck2 test //:hirsel-lir-test-it -- --test-arg=runtime_declarations_match_the_landed_c_header
.buck2/bin/buck2 test //:hirsel-rt-test-descriptors
.buck2/bin/buck2 test -c hirsel.native_filters=conformance/SEM/SEM-1-http-server-inline-errors-run.hir //:gate-native-changed
```

The complete ten affected corpus/native paths were inline-errors, inline, output-reuse, serial-handoff, stream-bodyless, stream-drop, stream-errors, stream-malformed, stream and wire, all under `conformance/SEM/SEM-1-http-server-*-run.hir`. The combined command used the exact path set for conformance/native selection. Only inline-errors received a successful isolated smoke retry; the other nine smoke selections are unexecuted, not claimed as native smoke passes.

## Measurement protocol

Ten seeded interleaved rounds, seed 20261022, include before/after hello, persistent keep and mixed sequence; unchanged owned keep and Rust hello/keep; plus separately identified current-production hello/keep/sequence controls. Every physical cell executes 1024 requests with exact wire validation and stdout `1024\n`. Mixed cells require 18,435,465 response bytes and SHA-256 `4ce3883f4796f85de8c2d36409f70207659a28ea52d8045c9fa03a056c27ab8c`. The original copied client still consumes the historical cases path; it and the byte-identical owned copy are both sealed.

CPU31 timing leases were unavailable during the nonblocking attempt; all locks were immediately released and no PMU/native timing samples were taken. Counts use pinned Cachegrind, loader and libc on server CPU13/client CPU14, with one CPU and 4G limits. Cachegrind Ir counts user-space instrumentation, not PMU/kernel instructions. Wall time under instrumentation is never used for acceptance. Separate ten-round allocation diagnostics execute the explicitly hashed diagnostic binaries, with production/default binaries used for instruction cells. The fixed original eleven-response scripted control runs ten repetitions per side in a separate seeded campaign. Sessions retain every raw command, environment, exit and binary/input hash. Failed cells are never silently excluded.

Allocation/frees, copied bytes, decrements, detach count, managed peak/live exit and supported region/pool/arena metrics are recorded independently of RSS. Runtime copied_bytes counts allocation/COW/deep-copy bytes, excluding ordinary byte stores and the replacement leaf's payload copy. Rust's unchanged binary provides allocator totals through pinned Memcheck, with Hirsel managed/RC/arena fields explicitly unavailable. Unavailable values are never interpreted as zero.

Append assembly and self-cost are retained in B. Both production and matched-before append bodies are 1090 bytes. Candidate append is 1250 bytes. Its scalar byte-store/backedge loop is removed, replaced by a checked wrapper call; the 72-byte runtime leaf calls memmove. Wrapper bodies and all corresponding disassembly are retained. Larger code size and fixed slice/wrapper/owner costs remain possible even after eliminating per-byte work.

## Measurement results and disposition

**HTTP NO-GO. Primitive GO.** The candidate removes scalar work for large initialized ranges, but significantly regresses the persistent static GET control. The serializer hunk is preserved only in B/rejected-http.patch and the frozen after/std snapshot; it is absent from the landed source. No rejected historical patch was executed. No tuning or second performance campaign was used to select a more favorable result.

| Comparison (left/right) | Median ratio of Ir | Paired bootstrap 95% interval |
| --- | ---: | ---: |
| after-hello / before-hello | 1.000609 | 1.000395–1.000611 |
| after-keep / before-keep | 1.049222 | 1.032647–1.091138 |
| after-sequence / before-sequence | 0.302992 | 0.293400–0.304525 |
| after-hello / rust-hello | 7.356664 | 6.920919–7.457712 |
| after-keep / rust-keep | 13.394694 | 13.286039–13.913041 |
| before-hello / production-hello | 0.999999 | 0.999998–1.000213 |
| before-keep / production-keep | 0.985621 | 0.944053–1.010554 |
| before-sequence / production-sequence | 0.991049 | 0.972577–1.017927 |
| fixed scripted after / before | 0.836803 | 0.836798–0.836804 |

Ratios divide within-session medians; intervals resample paired rounds (20,000 draws, seed 20261022). Keep regresses 4.92% (interval +3.26% to +9.11%); hello regresses 0.061%. Mixed improves 69.70%, and fixed scripted improves 16.32%. Before/current-production controls show no resolved infrastructure regression. Socket coalescing explains cell variation but does not justify accepting the positive keep-regression interval. The candidate adds slice/wrapper/source-credit work for short ranges: keep diagnostic decrements rise by about 10,230 per process while allocation/COW copied-byte counts stay similar. This explains a fixed-cost risk, not an exact additive attribution of every whole-process instruction.

| Participant | Median Cachegrind Ir/request | Allocation median [min,max] | Decrement median | Detach median | Copied bytes median | Managed peak median [min,max] |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| rust-hello | 4317.979 | 3100 [3100,3100] | unavailable | unavailable | unavailable | unavailable |
| rust-keep | 2096.107 | 30 [30,30] | unavailable | unavailable | unavailable | unavailable |
| production-hello | 31746.604 | 5120 [5120,5120] | 84993 | 1024 | 16384 | 272 [272,272] |
| production-keep | 27149.965 | 3925 [3828,4336] | 67896.5 | 1 | 16 | 1104 [944,1696] |
| production-sequence | 151525.706 | 10069 [9924,10300] | 80902.5 | 1 | 16 | 67328 [67328,68976] |
| before-hello | 31746.587 | 5120 [5120,5120] | 84993 | 1024 | 16384 | 272 [272,272] |
| before-keep | 26759.568 | 3863.5 [3698,4182] | 68071.5 | 1 | 16 | 1072 [992,1888] |
| before-sequence | 150169.345 | 10093.5 [9870,10458] | 80822 | 1 | 16 | 67328 [67296,68976] |
| after-hello | 31765.920 | 5120 [5120,5120] | 84993 | 1024 | 16384 | 272 [272,272] |
| after-keep | 28076.717 | 3883.5 [3794,4120] | 78301.5 | 1 | 16 | 1104 [640,1760] |
| after-sequence | 45500.045 | 10096.5 [9990,11186] | 90160 | 1 | 16 | 67296 [67296,77552] |
| owned-control-keep | 45839.166 | 25484.5 [25007,25615] | 82099 | 1025 | 629552 | 1056 [928,1744] |

All 100 Hirsel diagnostic cells have allocations=frees and live_bytes=0, region_high_water_bytes=0 and region_pool_high_water_bytes=0. Arena high water is unavailable. Rust hello has 3100 allocations/3097 frees per cell; Rust keep has 30/27, with the same 596 bytes in 3 blocks at exit in every Rust cell (548 reachable, 48 possibly lost, 0 definitely/indirectly lost; Memcheck reports 0 errors). These are unchanged-reference process totals, not Hirsel managed metrics or a claim of zero Rust live bytes. The initial summarizer incorrectly assumed zero Rust exit bytes; allocation-summary.log preserves that rejected analysis, and allocation-summary-2.log reports the actual values without rerunning cells. Complete per-metric min/max/medians, allocation/frees and separate RSS are in O/allocation-summary.json and every raw diagnostic cell.

| Append self Ir/process | Before | Candidate append | Candidate wrapper self |
| --- | ---: | ---: | ---: |
| hello | 727040 | 747520 | 0 |
| keep | 1,453,370 | 931660 | 511500 |
| sequence | 123,160,714 | 870701 | 469350 |

Append self excludes callees: the candidate wrapper column must also be considered, and memmove/runtime work is still outside both self counts. Hello creates fresh output backing, so it executes no replacement wrapper, yet its changed code has a small fixed overhead. Saved before assembly has the initialized byte store at +0x1e6 and the per-byte backedge at +0x1f3→+0x140 (including bounds/writability/RC). Candidate assembly replaces this region with bytes.slice plus the checked replacement wrapper call and has no scalar K-copy backedge. B/{production,before,after}-append.asm, replace-wrapper-{1,2,3}.asm, leaf.asm and their symbol/size JSON files preserve the actual final binaries' disassembly. Function names are encoded with double underscores; analysis matches the actual symbol rather than treating a missing unencoded name as zero cost.

The minimal product is still the unchanged 1024-request persistent `GET / HTTP/1.1\r\nHost: localhost\r\n\r\n` Hello-world server. Matched retained-scalar/Rust keep is **12.766315× [12.647053,13.081323]** Cachegrind Ir; hello is 7.352187× [6.916708,7.453173]. Candidate keep was 13.394694× [13.286039,13.913041]. None closes ≤3×. No native latency/throughput ratio is available because timing locks were unavailable. The remaining nine candidate HTTP native smoke cases are deliberately unexecuted after NO-GO; the final serializer has no source delta, and all primitive parity checks predate that serializer edit.

Reproduction commands (run from the owned implementation tree; logs in B):

```sh
python3 O/compile-stage.py production hello keep sequence
python3 O/compile-stage.py before hello keep owned sequence scripted-sequence
python3 O/compile-stage.py after hello keep sequence scripted-sequence
python3 O/freeze-consumed.py
python3 O/campaign.py counts
python3 O/campaign.py allocations
python3 O/scripted-campaign.py
python3 O/freeze-consumed.py --check
python3 O/evaluate.py
python3 O/summarize-allocations.py
```

O/REPRO.md and support-freeze.json preserve relocation instructions and byte-identical harnesses for replay after owned worktree cleanup. The original campaign scripts remain immutable execution evidence. Here `O/` denotes the absolute reproduction directory above, not a literal shell variable. Commands/statuses for each production and diagnostic compilation are preserved in O/{production,before,after}/*-status.json, with the actual environment in adjacent files. The baseline has six completed compilation cells; matched before has ten; matched candidate has eight. No completed cell was rebuilt or overwritten. B/production-compile-completed.log and production-completed-freeze.json preserve baseline evidence. The copied pinned Valgrind runtime and libc/loader are part of the toolchain freeze. The aggregate native timeout retains its action identity/stderr unchanged; no timeout was extended.

Primary session manifests are O/counts-session.json (120 accepted), allocations-session.json (120 accepted) and scripted-session.json (20 accepted). Each points to its complete raw directory, commands and exit statuses. All consumed input manifests verified unchanged before/after every campaign. Failed setup attempts remain in B; no invalid performance cell entered these three successful sessions.

## Prior art

Rust reference `f6c1107f8be13e48465e12c0fd26e6a39fe18e4f`, `library/core/src/slice/mod.rs:4280–4388`, `library/coretests/tests/slice.rs:2030–2088`, was read without mutation. The implementation follows copy_from_slice/copy_within's ordered range validation and overlap-safe copy structure. MIT/Apache-2.0 attribution to The Rust Project Contributors is retained in NOTICES.

Six deviations are required: immutable Bytes aliases survive through destination COW; MEM-14 plus cleanup replaces Rust panic; a valid Bytes interval replaces generic RangeBounds and invalid construction raises SEM-3 first; MEM-6 charges shared arena copying; the primitive is byte-only without generic finalization; length/capacity remain unchanged, unlike splice. History append's recursive overlap semantics are separate.

## Delivery

The HTTP serializer change is declined. The primitive/tests/docs are ready for landing; final SHA and numstat are recorded by the delivery artifact after integration. The unchanged minimal product remains 1024 persistent `GET / HTTP/1.1\r\nHost: localhost\r\n\r\n` requests to the Hello-world server, compared in the same session with unchanged Rust. Native wall-clock comparison remains open because exclusive timing leases were unavailable.

## Final path inventory

- `bench/micro/http/RESULTS-http-replace-range-impl-7.md`
- `crates/hirsel-driver/src/native/heap.rs`
- `crates/hirsel-driver/tests/it-stdlib/http_replace_range.rs`
- `crates/hirsel-driver/tests/it-stdlib/modules.rs`
- `crates/hirsel-engine/src/registry/review-table.txt`
- `crates/hirsel-engine/src/registry/section22.txt`
- `crates/hirsel-heapcheck/src/exec.rs`
- `crates/hirsel-interp/src/code.rs`
- `crates/hirsel-interp/src/eval.rs`
- `crates/hirsel-interp/src/library/dispatch.rs`
- `crates/hirsel-lir/src/runtime_abi.rs`
- `crates/hirsel-pipeline-tests/tests/rc/modules.rs`
- `crates/hirsel-pipeline-tests/tests/rc/rc_bytes_replace_range.rs`
- `crates/hirsel-rc/src/insert.rs`
- `crates/hirsel-rc/src/library/bytes.rs`
- `crates/hirsel-rc/src/library/mod.rs`
- `crates/hirsel-rc/src/library/value.rs`
- `crates/hirsel-rt-operations/src/descriptors.rs`
- `crates/hirsel-rt-operations/src/lib.rs`
- `crates/hirsel-rt/include/hirsel_rt_v2.h`
- `crates/hirsel-rt/src/text.rs`
- `crates/hirsel-rt/tests/it/bytes_replace_range.rs`
- `crates/hirsel-rt/tests/it/modules.rs`
- `docs/http-replace-range.md`
- `docs/spec/core-library.md`
- `native-tests/library/operations.tsv`
- `third-party/NOTICES.md`

No changes remain in stdlib/std@1/http1_parse.hir. LLVM/Cranelift emission and general cleanup/scheduler/shadow implementations were audited or consumed unchanged. Same-owner arena source construction remains the explicit PROV-4 limitation above.
