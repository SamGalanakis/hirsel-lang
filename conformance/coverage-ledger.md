# SEM / MEM / DROP / FAIL coverage ledger

Audit date: 2026-10-22. Source baseline:
`dd14443a155555e8af920a5d731d1a737cefc85d`, after cutover `69412fd9`,
loan-lowering `2b0e271fc5e7` and HTTP freeze/replay `1641553b` / `dd14443a`.
This audit changes documentation and one interpreter test, not language semantics,
compiler lowering, runtime ABI, memory budgets or validators.

**Resolved** below closes the stated historical absence or narrow coverage claim.
**Open oracle** means a finite witness exists but does not establish the universal
property. **Current defect** means an executable expected result still fails.
**Unspecified** means a language decision, not an implementation ticket.
Owners name the latest known lane where one is recorded, including settled lanes.
Otherwise the table names the responsible maintainer role: no live assigned lane
was found in the current plan. These are explicit ownership gaps, not invented
implementation assignments; spec decisions remain with spec maintainers.

## Semantics

| Former GAPS entry | Current rule and disposition | Exact witness / residual oracle | Latest owner |
|---|---|---|---|
| Conversion/division signatures and exact-division failure | SEM-1 / core numeric contracts; resolved examples | `SEM/SEM-1-exact-division-{ok,fail}.hir`; interpreter `library::every_integer_conversion_pair_and_all_four_contracts` and `floor_division_divisor_signed_mod_and_exact_division` | numeric semantics maintainers |
| `float.to_i32` errors and sum tree | SEM-2; resolved concrete coverage, open universal rounding oracle | `SEM/SEM-2-to-i32-contract.hir`, `SEM-2-wave2-f32-sum-success.hir`; interpreter `library::float_int_conversion_and_explicit_fusion`. All-input correct rounding and emitted reduction graphs still require an independent arithmetic/IR oracle | float semantics maintainers |
| Scalar/grapheme types, terminal positions, UTF-8 and Unicode 16 | SEM-3; resolved examples, open universal construction/table oracle | `SEM/SEM-3-graphemes-source-run.hir`, `SEM-3-positions-terminal.hir`, `SEM-3-utf8-run.hir`; pinned Unicode conformance data remains the universal table oracle | Unicode/std maintainers |
| Map replacement order | SEM-5; resolved finite witness | interpreter `library::ordered_collections_replace_without_reordering_and_copy_independence`; seed/physical-bucket universal claims remain open | collection maintainers |
| Floating universality and SipHash-1-3 | SEM-2/5; open oracle, not absent APIs | [SEM/UNTESTABLE.md](SEM/UNTESTABLE.md): exhaustive small-width arithmetic, independent IEEE oracle, published SipHash vectors and instrumented seed/placement model. `library_callbacks::hash_random_and_test_seeds_vary_independently` is a registered finite control, not run by this audit | numeric/hash maintainers |
| Undeclared correctly-rounded transcendental APIs | SEM-2 / named std contracts; contract boundary | Test each declared function's stated error contract; no invented “all transcendental operations correctly rounded” requirement | spec/std math maintainers |
| Flat bytes (old #1) | MEM-12; resolved feature absence | `MEM/MEM-12-flat-bytes-field-run.hir`; native filtered control below. Physical flat bytes are private; SER-2 is a different contract | flat-layout maintainers |
| Shape-driven generation (old #5) | CT-3/5; resolved feature absence | `CT/CT-3-shape-record-run.hir`, `CT-5-generated-ownership-{ok,bad}.hir`; generator landed `45215af7` | compile-time generation maintainers |
| Ratified provisional markers / “section 22” | Historical references; resolved documentation | Use current named rules and [core contracts](../docs/spec/core-library.md); no new rule or blanket provisional exception | spec maintainers |

## Memory

| Former GAPS entry | Current rule and disposition | Exact witness / residual oracle | Latest owner |
|---|---|---|---|
| MEM-3/6 exact scratch/frame and arena boundaries (old #16) | Split: LANG-10 / MEM-3 frame and scratch tables **unspecified**; MEM-6 arena table **settled**, three current checker defects | [Normative arena table](../crates/hirsel-rt/README.md#bounded-arenas); exact source failures below. An implementation recursion limit is not a language constant | spec maintainers for tables; checker-precision / ca-integrate history for PROV-4 |
| MEM-4/10 builder publication, relocation, held children, idle cap | Resolved absence of instrumentation; broader compiled-path oracle open | Runtime `regions::{an_unpublished_builder_grows_in_place_then_relocates_with_its_contents,a_published_builder_never_grows,a_failing_exit_still_releases_every_heap_edge_and_meets_the_pool_cap}`; `arena_lists::heap_elements_of_relocated_and_copied_lists_are_released_once`. These assert physical data/counts, unlike source output | region/runtime maintainers; rss-throughput-2 retained existing pool policy |
| MEM-5 cleanup effect pairs | Resolved static examples; open universal allocation oracle | `MEM/MEM-5-{generic,destructor-field}-cleanup-{ok,bad}.hir`; cross-definition effects stay signature-based. Instrument every early failure/cancellation and recursive generic summary separately | noalloc-contract maintainers |
| MEM-8 full integer-lane boundaries | One bounded hole filled; universal/backend scope open | New `library::simd_add_checks_every_integer_width_and_each_lane`: 10 fitting boundary vectors + 45 overflow/underflow vectors across all three lane positions; existing schema/splat/zero-width control retained | coverage-ledger-2 for this control; vector maintainers for broader coverage |
| MEM-9 unchecked/foreign/raw memory (old #6) | Surface shipped; operation-specific vectors and unsafe review open | Interpreter `library::unchecked_add_detects_overflow_without_inventing_a_language_failure`; driver `foreign::{unsafe_ffi_metadata_errors_are_tool_errors,native_calls_c_with_scalars_a_byte_view_and_a_released_handle}` both pass. Written unsafe aliasing contracts and environmental crash paths need sanitizer/process oracles | FFI/unsafe maintainers |
| MEM-12 field-only growable and flat format (old #1) | Resolved static/feature history; physical oracle open; composition/ABI unspecified | `MEM/MEM-12-growable-last-{ok,bad}.hir` and flat-byte witness. Full pointer-free layout, copy cost and cross-task representation need layout/allocator controls; wire golden vectors do not prove them | flat-layout maintainers; spec maintainers for MEM-7 composition |
| MEM-11 physical sharing and reclamation | Partial model coverage; open weak-memory/universal oracle | Runtime `roots::a_root_count_stays_exact_through_the_overflow_table` and nested-root walk controls; latest unique-copy/freeze repair `1641553b` remains owned by http2-freeze-2, no duplicate rewrite | http2-freeze-2 / mc-ordered / mc-free |
| MEM-14 generation wrap | Resolved absence of a boundary model; exhaustive small-generation model remains open | `library::slots::tests::exhausted_generation_retires_the_vacancy` directly constructs generation MAX−1, reaches MAX, retires it and checks stale IDs; this is not exhaustive generation-state enumeration | slot/runtime maintainers |

The 8 MiB cap is a settled per-worker idle-pool bound, not a new allocation budget.
[MEM/UNTESTABLE.md](MEM/UNTESTABLE.md) retains universal address/count/layout obligations.
Complete layout composition, native data ABI, logical frame charges and structural
scratch levels stay in [Unspecified](../docs/spec/index.md#unspecified).

### Current arena defects

The 31-case selection **executed 31**, with **28 passing / 3 failing**, exit 32.
All three are inherited checker refusals before execution, already in
[known-failures](../tools/known-failures.tsv); none was removed or treated as
an arena-accounting failure:

| Minimal registered program | Expected | Actual on matched baseline |
|---|---|---|
| [MEM-6-exact-cap-fits-run.hir](MEM/MEM-6-exact-cap-fits-run.hir) | `released\nfits\n`, exit 0, exactly 300 logical bytes | PROV-4 line 18 |
| [MEM-6-exhausted-callback-cleanup-run.hir](MEM/MEM-6-exhausted-callback-cleanup-run.hir) | `released\nout of space\n`, exit 0 | PROV-4 line 19 |
| [MEM-6-outer-arena-exhaustion-run.hir](MEM/MEM-6-outer-arena-exhaustion-run.hir) | `outer out of space\n`, exit 0 | PROV-4 line 9 |

Each refuses an arena value as requiring heap placement. Minimal repro command:
`.buck2/bin/buck2 test -c hirsel.conformance_paths=conformance/MEM/MEM-6-exact-cap-fits-run.hir //:gate-conformance-selected-shard-1`
(substitute either other exact path to reproduce it). Latest related ownership:
checker-precision / ca-integrate cutover history; current routing retained with
project owner. No active integration repair is undertaken here.

## Destruction and failure

| Former GAPS entry | Current rule and disposition | Exact witness / residual oracle | Latest owner |
|---|---|---|---|
| Unit-only drop, bug failures, explicit/structural noalloc | DROP-2/3 / MEM-5; settled, concrete coverage present | Generic/field cleanup pairs above; runtime `finalizers::finalizer_failures_keep_the_first_as_primary_and_release_everything_once` and `an_earlier_language_failure_stays_primary_during_cleanup` | cleanup/noalloc maintainers |
| Cross-module forget and external resource destructure (old #4) | DROP-4 / LANG-13 / PROG-2; resolved fixture absence | `DROP/DROP-4-external-forget-{ok,bad}.hir`, `DROP-4-package-module-forget-bad.hir`; `LANG/LANG-13-external-destructure-{ok,bad}.hir` use real package manifests, not dotted declarations. Fixture support landed `7df35dc0` | package/resource maintainers |
| Cancellation drop traces / M2 | DROP-2/3 / CONC-3; scheduler exists, finite controls present, universal oracle open | `CONC/CONC-3-cancellation-shields-drop-run.hir`; runtime `finalizers::{finalizers_run_shielded,compiled_finalization_shields_until_its_end,compiled_handle_close_runs_shielded_and_reports_a_failing_close}`. Every injected schedule/Io implementation and allocation-free cleanup path is not proved | scheduler/cleanup maintainers; mc-ordered / mc-free for production scheduling |
| Unsafe-FFI drop termination (old #6) | FAIL-3; resolved absence of runtime subprocess control | Runtime `finalizers::{a_failing_unsafe_ffi_finalizer_ends_the_process,a_failing_unsafe_ffi_field_ends_the_process_even_inside_a_safe_value,a_failing_derived_unsafe_ffi_finalizer_ends_the_process}`; package metadata-to-descriptor end-to-end coverage remains a separate oracle scope | FFI/runtime maintainers |
| Failure/SourceSite/FailureKind fields (“section 22”) | FAIL-1/2 and core concurrency types; settled, source witness present | `FAIL/FAIL-2-isolate-failure-fields-run.hir`; exact all-layout ABI and all schedule cause chains remain model obligations | failure/runtime maintainers |
| Depth / bounded OutOfSpace (old #16) | Split unspecified LANG-10/MEM-3 vs settled MEM-6 | Arena defects and unspecified table decisions above; no invented constant and no language-Depth credit for a tooling-limit test | spec maintainers / checker-precision history |
| LANG-9 check selection and entry/capability denial (old #4) | LANG-9 / PROG-3 / CAP-1; resolved support absence | `LANG/LANG-9-native-test-check-{run,fail}.hir`; interpreter `library_launch::{invalid_and_unavailable_entries_never_query_a_body_or_partly_grant,explicitly_configured_unsafe_is_granted_and_configuration_is_deterministic}`; CLI controls below | test/launch maintainers; pk-selection for selection policy |
| FAIL-4 reacquisition / non-sendable visibility (old #22) | Resolved for Mutex handles under current CLO-3 / CONC-2 | `FAIL/FAIL-4-owned-payload-dead-run.hir` passes with legal copied handles; interpreter `library::dead_mutex_releases_updated_payload_and_preserves_original_failure` checks updated payload release and exact original cause. This does not authorize arbitrary non-sendable captures | tasks-gaps / mutex maintainers |
| Failure/cancellation cleanup for every frame | FAIL-3 / DROP-2; open oracle | Runtime finalizer/region controls are finite; independent frame/RC instrumentation and scheduler injection across every exit are still needed. [FAIL/UNTESTABLE.md](FAIL/UNTESTABLE.md), [DROP/UNTESTABLE.md](DROP/UNTESTABLE.md) | cleanup/scheduler maintainers; mc lanes retain their proof work |

## Tasks history

Only #520546 is newly closed here: the exact unchanged
`tasks_e2e_client_builds_with_llvm_in_bounded_memory` built the original current
`examples/tasks/e2e/hirsel.json` with LLVM, **1 passed**, exit 0, assertion
**peak RSS < 1536 MiB**. It took 83.17 s in this one run; this is not a comparative
performance measurement. Successful execution prints no peak. Historical 1554 MiB
and same-compiler original 1587 MiB failures remain recorded in tasks/GAPS;
this audit attributes closure to the post-cutover matched toolchain, not one
unmeasured optimization. The client remains interpreted in the e2e harness by
choice; no harness policy change is needed to close the compiler regression.

The following already-fixed entries are **retained history**, not fresh test
claims. Owner: tasks-gaps / st-app, with their original implementation and cleanup
SHAs in [tasks/GAPS](../examples/tasks/GAPS.md). Registered app oracles are CLI
`tasks_package_tests` (expects 25 package tests per interpreted/LLVM run),
`tasks_end_to_end_interpreted`, `tasks_end_to_end_llvm`, and CLI
`signals::a_granted_run_consumes_an_interrupt_and_returns_ok` for signal delivery.
These historical app/signal oracles were not re-run in this audit.

| Historical message | Rule / declared contract and retained witness |
|---|---|
| #513132 | CONC-3 / signal recording: `signals::a_granted_run_consumes_an_interrupt_and_returns_ok`; stream shutdown in `tasks_end_to_end_llvm` |
| #514473 (dates) | SER-1/2 / time.Date JSON hooks: `tasks_package_tests`, model JSON/date validation |
| #520100 | HTTP HEAD/204/304 framing contract: app `httpd.send` uses `respond`; SDK framing controls belong to HTTP owners |
| #513862 | LANG-9 test.tasks: `tasks_package_tests`, hub/save-then-publish tests |
| #513128 | HTTP stream contract: `tasks_end_to_end_llvm` holds streams across shutdown/restart |
| #513667 | HTTP max_headers: same exact e2e client uses std HTTP with configured limits |
| #513157 | LANG-13 String literal patterns: protocol-name parsers exercised by package/e2e tests |
| #513475 | MODE-4 / MEM-11 / CONC-4: Services carries Mutex/Shared handles, package/e2e tests |
| #513531 | CLO-3 / CONC-2 listener capture: app.serve takes the listener, e2e server startup |
| #513641 | Declared listener restart contract: `tasks_end_to_end_llvm` restarts on the same port |
| #514255 | OWN-1 / PROV-4 borrowed split result: app parser in package/e2e tests |
| #514323 | MEM-11 freeze of taken task capture: current Shared controls; original regression owner retained |
| #514473 (formatter, Inline, HTML, nested JSON) | LANG-1 formatting / MEM-7 repeat literal / std HTML / SER-1 codec dispatch: original `597505ae` / `98a38054` history retained; app request parser is gone, no current parser defect invented |
| Task name and local shadow restrictions | LANG-7 / LANG-4: intentional language rules, not gaps; no implementation ticket |

## Reproduction and accounting

The exact 33 reclassified source cases and their twins are listed in
[coverage-ledger.paths](coverage-ledger.paths). The initial 31-case result is kept
as a failure, not laundered into a green by excluding the three PROV-4 cases.
The additional external-destructure pair is selected separately.

```sh
.buck2/bin/buck2 targets 'root//:'
.buck2/bin/buck2 test -c hirsel.conformance_paths="$(paste -sd '|' conformance/coverage-ledger.paths)" //:gate-conformance-selected-shard-1
.buck2/bin/buck2 test //:hirsel-interp-test-it -- --test-arg=library::
.buck2/bin/buck2 test //:hirsel-interp-test-it -- --test-arg=library_launch::
.buck2/bin/buck2 test //:hirsel-interp-unittest -- --test-arg=exhausted_generation_retires_the_vacancy
.buck2/bin/buck2 test //:hirsel-rt-test-it -- --test-arg=regions:: --test-arg=arena_lists:: --test-arg=finalizers:: --test-arg=roots::
.buck2/bin/buck2 test //:hirsel-cli-test-examples-shard-1 -- --test-arg=tasks_e2e_client_builds_with_llvm_in_bounded_memory --test-arg=--exact
```

The brief's literal `targets 'root//:*'` is rejected by this Buck2 (exit 3,
invalid target name `*`). `targets 'root//:'` discovers actual targets; full
native target discovery additionally needs `-c hirsel.full=true`, selected
conformance discovery needs the `hirsel.conformance_paths` configuration.
The verified shard-1 Rust launcher routes a supplied filter to that selection
rather than running its ordinary cost partition. No whole campaign was run.

Execution ledger and binary identities follow below. Every TestRun in the event
logs has `cache_hit=false`; completed semantic tests actually executed remotely.
One additional FFI attempt stopped during input upload (size mismatch), executing
zero tests; its unchanged retry passed 2/2. That infrastructure attempt supplies
no semantic evidence. Build-cache reuse is
identified separately from the nonzero test executions and their captured counts. A build result
or zero-test result is never substituted for execution. Native pending is empty
(no noncomment rows) and is deliberately unchanged. Known failures change only
by the exact successful tasks guard row; the unselected historical rows stay.

| Selection actually run | Executed result | Exit / cache |
|---|---|---|
| Initial exact source selection (first 31 manifest paths) | 31 executed; 28 pass / 3 PROV-4 failures listed above | 32; executed; build actions cached |
| External-destructure pair (last 2 paths) | 2 executed / 2 pass | 0; executed; build actions cached |
| Interpreter `library::` before addition | 25 pass | 0; executed; build actions cached |
| Interpreter launch controls | 2 pass (in initial new-control run) | Initial selection exit 32 only because of the test assertion below |
| New SIMD control, corrected and final formatted source | 1 pass; final binary has 99 registered tests vs baseline 98 | 0; executed remotely, final formatting-only run reused compiled harness |
| Slot retirement boundary | 1 pass | 0; executed; build actions cached |
| Runtime regions / arena_lists / finalizers / roots | 45 pass | 0; executed; build actions cached |
| CLI selection, Test entry, denied Io entry, projected grants | 4 pass | 0; executed remotely |
| Foreign metadata and native C wrapper/handle control | 2 pass | 0; remote retry after upload refusal |
| Original tasks LLVM memory guard, once | 1 pass, original client and <1536 MiB bound | 0; executed remotely |
| Filtered native smoke, three sources | 3/3 pass, each 4 configurations plus real Io recording/replay and Cranelift executable; 11 image-runner runs per source | 0; executed remotely |
| Filtered Cranelift parity, same three sources | 3/3 selected, LLVM/Cranelift observations agree | 0; executed remotely |
| Spec definitions, cited code rule IDs, relative spec links | 3 pass | 0; executed remotely |
| GAPS/ledger relative links, source registrations, twin and failure-ledger census | 31 links, 33 source paths/twins, 33 named Rust registrations; known failures 64→63, native pending 0→0 | Audit below |

The first new SIMD test run had **2 pass / 1 fail**, exit 32: its assertion
incorrectly demanded the description `Overflow` instead of the implementation's
`SIMD lane overflow`. The normative result is SEM-1 at the call site. The
corrected test asserts that rule/site, retaining all boundary/lane vectors;
no runtime behavior was changed. Before and failure logs are retained separately.

Exact additional commands (the Rust filters select named tests, not whole shards):

```sh
.buck2/bin/buck2 test //:hirsel-driver-test-it-shard-1 -- --test-arg=foreign::unsafe_ffi_metadata_errors_are_tool_errors --test-arg=foreign::native_calls_c_with_scalars_a_byte_view_and_a_released_handle
.buck2/bin/buck2 test //:hirsel-cli-test-cli-shard-1 -- --test-arg=testing::selection_is_explicit_filtered_and_failure_isolated --test-arg=bare_run_refuses_an_io_main_identically_in_every_engine --test-arg=bare_run_executes_a_test_main_in_every_engine --test-arg=source_projection_controls_distinguish_granted_and_empty_io
.buck2/bin/buck2 test //:gate-native-smoke -- --test-arg=--filter --test-arg=MEM-12-flat-bytes-field-run --test-arg=--filter --test-arg=CONC-3-cancellation-shields-drop-run --test-arg=--filter --test-arg=FAIL-4-owned-payload-dead-run
.buck2/bin/buck2 test -c hirsel.full=true //:gate-native-cranelift -- --test-arg=--filter --test-arg=MEM-12-flat-bytes-field-run --test-arg=--filter --test-arg=CONC-3-cancellation-shields-drop-run --test-arg=--filter --test-arg=FAIL-4-owned-payload-dead-run
.buck2/bin/buck2 test //:hirsel-conformance-test-spec_links -- --test-arg=every_spec_rule_is_defined_once_with_a_statement --test-arg=every_rule_id_the_code_cites_is_defined --test-arg=every_relative_spec_link_resolves
```

The SIMD control starts from Rust's last-fitting/first-overflow boundary pattern:
`rust-lang/rust` commit `a7c37f9f0ee5e93396106e9de20f4a86b4154e63`,
`library/core/src/num/int_macros.rs:586–587` and
`library/core/src/num/uint_macros.rs:944–947`. Hirsel-specific changes: extend the
pattern to ten scalar widths and each vector lane, include signed underflow, and
expect a deterministic SEM-1 language failure at the call instead of Rust's
`None`. Bounds come from explicit Rust integer constants, not the same
`integer::binary` implementation used by SIMD. No protocol or runtime algorithm
was ported or changed.

### Exact artifact identities

The original guard artifacts were materialized again from the baseline build
cache in an owned detached checkout, without re-running the guard. Their full
SHA-256 values agree with the original event-log output prefixes:

- Guard CLI: `cdb72b1388de188ee672b0dbead82e693b4bf8b51a5985211bb0c37ef28a138f`.
- Guard std image: `b21af6e196015c8ef784828776959720c85152f0fca6d54d3d32df029fabae71`.

Baseline compiler/std/runtime sources are the baseline Git SHA above. The tasks
memory guard used the unmodified test body at `crates/hirsel-cli/tests/examples.rs:2616`.
Buck2 immutable action identities below bind the exact executed inputs and outputs;
these are action SHA-256 keys (including their encoded size), not executable hashes:

| Executed build artifact | Action identity | Output digest prefix in event log |
|---|---|---|
| Guard's checked CLI | `b185d55cbcc02f5ab65ee9f1defef1fa5ced5240d330bae574e6073c7393a1c1:142` | `cdb72b13` |
| Guard's examples harness | `77eb93f921b30dc9168029523cc4a8b63bff4801a1c7793ce8a26247793727a0:142` | `1e7ce54d` |
| Guard's matching std image | `51c412cc7e88a16f10b723e59d4938425c3a4b1db7188f2b7df80f853f4659e3:142` | `b21af6e1` |
| Initial conformance harness | `a7cc3c41bce2a8531ca489c705fd8a3af5df8790edfa453d34438fdbdffa8c42:142` | `1050a0d7` |
| Runtime model harness | `a4e14152fe18dcff77afb3ff19ebb4759b67aa2b6800bd8ef280231527292629:142` | `3ff180cd` |
| Original interpreter harness | `2bd770659c7de35cb4689c1326ab57f563f79e676addc11d0d4787a26e4d9a88:142` | `2ae7597d` |
| Corrected SIMD harness | `d5f528de558e6eaf0463e6943a589ff0fb015877dccac5cc900f407c3962b815:142` | `3a0105cb` |

Materialized later native/CLI controls use these full executable SHA-256 values
(checked platform `bd5018e3f47c9a34`); they are not substituted for the guard's
earlier action identity:

| Artifact | SHA-256 |
|---|---|
| CLI used in four launch/selection controls | `e1a2a85fe8dd48d928f56c135bba290511707f1e340ce6ef389c29b2a94b710a` |
| Native harness for smoke/parity | `a033911280f511f8034970e3cffef9db935e4f5cc4b57a7741d5df2bcbcb9065` |
| Final formatted SIMD harness | `d1b17137eda93c93e96bd10e14884a8f98ddfb9fff57b24a9f311f907311c52a` |
| Runtime model harness | `3ff180cd21a61f95515011f65d78585f66cbb29a0ee0d111415a9fb6346c72a6` |
| x86-64 default runtime archive | `18b330dc022603617fc9d6fec5aa7ef1dad18ebebe14e5985e40fc2743f3aa2f` |
| x86-64 diagnostic runtime archive | `85b541bc6e836f1283bc94a38d1c52661ff0f8182bd0adccddb328eb694ddd9e` |

Audit method: resolve every manifest path and its `// twin:` to a current corpus
file; verify each named Rust control has a `#[test]` registration; compare all
known-failure IDs in this selection with those registrations and retain the three
reproduced arena failures. Native corpus pending contains zero obligations before
and after. No broad known-failure deletion, native pending waiver, mc proof work,
performance goal or adoption gate is claimed by this closure.
