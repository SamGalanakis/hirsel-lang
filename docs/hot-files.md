# Concurrent registrations

Add ordinary test modules to `modules.rs` beside the harness's `main.rs`.
Each line is exactly `mod name;`. The harness includes the declarations at its
crate root, so existing module paths, test names, guard setup and Buck2 test
shards stay the same. Keep attributes and `#[path]` support modules in `main.rs`;
they are not independent lines. Both Cargo and Buck2 compile the include, and
`tools/buck2/fmt.py` checks each registered module as a rustfmt root, without
requiring independent declarations to be reordered.

The `hirsel` merge driver in `tools/buck2/merging.py` combines both these lists
and `tools/known-failures.tsv` as whole-line sets. For base B and sides O/T the
result is `(B ∩ O ∩ T) ∪ (O − B) ∪ (T − B)`: a baseline row survives only if
both sides keep it, and either side's new rows survive. Thus deleting resolved
failure R while another lane adds S yields S. Git's built-in `merge=union`
would bring R back, silently hiding a fixed test. This driver protects ordinary
merges and rebases as well as landing, which is why it is preferable to a
push-only ledger check.

Bootstrap (`python3 tools/buck2/prepare.py`) and `tools/land` install the driver
in the clone's Git config. To configure it before a manual merge or rebase
without preparing build tools, run:

```sh
git config merge.hirsel.name 'files many lanes add to (tools/buck2/merging.py)'
git config merge.hirsel.driver 'python3 tools/buck2/merging.py %O %A %B %P'
```

Custom drivers require this local configuration; `.gitattributes` alone does
not install them. The two former union patterns both use `merge=hirsel`:
`crates/*/tests/*/modules.rs` and `tools/known-failures.tsv`. The set operation
reuses the removal rule from `sorted_stretch`
(`tools/buck2/merging.py` at `37dae771`, lines 193–194), over the entire file so Git's conflict-hunk boundaries cannot change its result.
It retains ours' line order followed by new lines from theirs, removes identical
duplicates and writes final newlines. Declaration and ledger row order has no
semantics. Different rows with the same ledger ID remain visible to validation.
`tools/check-hot-files`, run by `tools/land` on every rebased tree including
push-only retries, rejects duplicate declarations and duplicate ledger IDs.
The ledger uses main-runs' existing four-column/date validation. Do not add
comments, attributes or blank lines to the declaration lists.

Publish benchmark measurements, raw sessions, profiles and reproduction scripts
under `/workspace/code/hirsel-briefs/<lane>/`. LangArena and the tasks app default
to external session directories. Keep only a short result row or archive link in
an existing results file; do not add per-lane result files to the public repo.
Never union-merge Markdown tables: adjacent rows can have different headings,
units, sessions and interpretation.

BUCK inventories and related build inputs are already ignored and derived by
`tools/buck2/sync.py --if-stale`. Do not commit them. Landing can replay legacy
commits with conflicts in generated files: build inputs are untracked and
regenerated before checks; `docs/api.md` keeps origin/main's generated page,
and its sole writer, `tools/main-runs`, regenerates it after landing. Source
conflicts still abort the rebase. No lane commits a generated page.

Corpus passing/pending lists use the existing `hirsel` driver, installed by
bootstrap and landing. They merge by path even for unsorted appends alongside
comments. Either side's removal wins over an unchanged row; conflicting changes
to one path's expectation remain conflicts. Entries are sorted after merging.

## Inventory

Measured on origin/main `27fea8b93884`, 2026-10-22, with
`git log 27fea8b93884 --since=2026-10-19T00:30:00Z --format= --name-only`
(the three-day window at inventory time), counting commits
per path (including historical paths removed or moved since those commits).
Counts measure touches, not just additions; the high-frequency candidates were
also inspected with `--numstat`. Implementation code such as CLI `src/main.rs`
is excluded: its edits are not independent registrations.

| File | Commits | Resolution |
| --- | ---: | --- |
| `conformance/passing.txt` | 176 | Path-set merge accepts unsorted appends/comments; preserves removals |
| `third-party/NOTICES.md` | 96 | Existing append-only merge driver; preserve notice prose/attribution |
| `tools/known-failures.tsv` | 84 | Three-way line set; removals survive; land rejects duplicate IDs |
| `docs/api.md` | 42 | Existing main-runs generator; legacy rebase conflicts keep main's writer |
| `crates/hirsel-engine/src/registry/review-table.txt` | 35 | Existing semantic sorted-list merge |
| `BUCK` | 31 | Already removed from Git, generated on demand |
| `native-tests/library/operations.tsv` | 30 | Existing semantic sorted-list merge |
| `crates/hirsel-driver/tests/it-stdlib/main.rs` | 18 | Ordinary declarations isolated into independent `modules.rs` |
| `crates/hirsel-driver/tests/it/main.rs` | 17 | Line-set `modules.rs` |
| `crates/hirsel-driver/tests/it-scaling/main.rs` | 10 | Line-set `modules.rs` |
| `crates/hirsel-rt/tests/it/main.rs` | 9 | Line-set `modules.rs` |
| `crates/hirsel-run-config/tests/it/main.rs` | 8 | Line-set `modules.rs` |
| `tools/buck2/BUCK` | 7 | Build rules, not an append-only test registry; unchanged |
| `crates/hirsel-pipeline-tests/tests/front/main.rs` | 6 | Line-set `modules.rs` |
| `crates/hirsel-authority/src/corpus/passing.txt` | 6 | Path-set merge, accepting unsorted appends |
| `crates/hirsel-rt/tests/descriptors/main.rs` | 5 | Actual test bodies, not a registry; unchanged |
| `crates/hirsel-ownership/src/tests/{passing,pending}.txt` | 5 each | Path-set merge, accepting unsorted appends |
| `crates/hirsel-authority/src/corpus/pending.txt` | 5 | Path-set merge, accepting unsorted appends |
| `crates/hirsel-llvm/tests/it/main.rs` | 5 | Line-set `modules.rs` |
| `bench/langarena/RESULTS.md` | 5 | Historical; new reports shard by session |
| `bench/tasks-perf/RESULTS.md` | 4 | Historical; explicit per-session output |
| `bench/apps/tasks/RESULTS.md` | 3 | Historical; runner shards by session |
| `third-party/BUCK` | 3 | Already ignored, generated on demand |
| `crates/hirsel-interp/tests/it/main.rs` | 3 | Line-set `modules.rs` |
| `crates/hirsel-free/tests/it/main.rs` | 3 | Line-set `modules.rs` |
| `crates/hirsel-{validate,parser,lir,conformance}/tests/it/main.rs` | 2 each | Line-set `modules.rs` |
| `crates/hirsel-pipeline-tests/tests/rc/main.rs` | 2 | Line-set `modules.rs` |
| `crates/hirsel-{object-storage,code-store}/tests/it/main.rs` | 2 each | Actual test bodies, not registries; unchanged |
| `crates/hirsel-driver/tests/it-precheck/main.rs` | 2 | Historical path, removed on main |
| `crates/hirsel-modes/src/ownership/tests/passing.txt` | 2 | Historical path, moved to hirsel-ownership |
| `crates/hirsel-modes/src/authority/corpus/pending.txt` | 2 | Historical path, moved to hirsel-authority |
| `bench/micro/http/RESULTS.md` | 2 | Historical; runner already requires output |
| `tools/toolchain-dist/BUCK`, `tools/buck2/native-toolchain/BUCK`, `bench/micro-json-reference/BUCK` | 2 each | Build rules, not test inventories; unchanged |
| `crates/hirsel-{run-schema,remote,heapcheck,engine}/tests/it/main.rs` | 1 each | Line-set `modules.rs` |
| `crates/hirsel-driver/tests/it-keyed/main.rs` | 1 | Line-set `modules.rs` |
| `crates/hirsel-driver/tests/it-image/main.rs` | 1 | Actual test body, not a registry; unchanged |
| `conformance/pending.txt` | 1 | Path-set merge, accepting unsorted appends |
| `tools/h2spec/passing.txt` | 1 | Static baseline, not shared lane registration; unchanged |
| `tools/compiler-bench/RESULTS.md`, `bench/micro/net/RESULTS.md` | 1 each | Historical; new reports shard |
| `/workspace/code/hirsel-briefs/port-3/repo-evidence/bench/langarena/RESULTS-port-3.md` | 1 | Already sharded |
| `tools/buck2/toolchains/BUCK` | 1 | Build rules, unchanged |
| `BUCK.unsynced`, `third-party/BUCK.unsynced` | 1 each | Constant bootstrap stubs, unchanged |

| `tools/buck2/workspace-metadata.json` | 10 | Already ignored, generated on demand |
| `tools/buck2/test_sizes.bzl` | 6 | Already ignored, generated on demand |
| `third-party/Cargo.toml`, `third-party/Cargo.lock` | 3 each | Already ignored, generated on demand |
| `tools/native-toolchain/pins.bzl` | 2 | Already ignored, generated on demand |
| `tools/buck2/exec_sizes.bzl`, `third-party/lib.rs` | 1 each | Already ignored, generated on demand |
| `stdlib/std@1/hirsel.json` | 41 | Existing structural manifest merge driver |

The generated files tracked by `tools/buck2/generated.py` are the source of truth
for landing's generated-file handling. Root/third-party BUCK, Cargo dependency
inventory, sizes, native pins and workspace metadata are all on that list.

## Verification

`tools/tests/hot_files.py` creates scratch branches and checks add/add,
delete/add and delete/delete for both former union patterns, through real
merges and rebases in both orders. System/global Git configuration is disabled;
the scratch clone installs the same local driver as bootstrap and landing.
The delete/add case failed in all four operation/order combinations with the
old union attributes. `tools/tests/merging.py` exhausts all eight possible
base/ours/theirs row memberships, checks line order/final newlines and covers
the corpus append shape, twin rows, removals and conflicting expectations.
Run `python3 tools/tests/hot_files.py` and `python3 tools/tests/merging.py`;
main-runs also executes these host-side Git harnesses in its tool-test stage.
`tools/tests/land.py` covers
replaying multiple generated-file conflicts, regenerating an ignored build
inventory, refusing source conflicts, moved-main retries and failed checks.
The stdlib pre-switch harness listed 438 tests (original registry bodies plus
unused draft module-list files; not a pristine-checkout baseline).

All 19 migrated harnesses built under Buck2 and listed 3399 tests.
The stdlib listing is byte-identical before and after the registry switch:
438 tests. Two throwaway branches based on the candidate each added a stdlib
module, a ledger row and an unsorted comment-adjacent corpus batch. Rebases
passed in both directions, retaining all four corpus entries and preserving
one removed baseline path. Buck2 then executed both new driver/std client
tests: 2 passed, 438 filtered out (440 tests in the proof harness).

The candidate source manifest and proof source manifest were hashed before
build/test and checked unchanged afterwards. Focused tool checks pass:
4 registration/formatting checks, 4 corpus merge checks, 6 landing checks,
and 4 task report checks. Temporary proof modules and ledger rows are not
part of the landed change.

## Resurrected-row audit (2026-10-22)

Audited origin/main `5f7d6f73`: 46 failure rows, zero exact current rows found
among deletions in the ledger's first-parent history. Comparing IDs also found
eight matches: four owner-field repairs in `ede32cc4`, one expanded corpus
reason in `b8c1ce45`, and three different historical reasons in `28f312b5` and
`1fdf0d3b`. Those are edits or later reports, not proof of resurrection; they
remain in the ledger. No additional row removal is justified at this revision.

The nine rows resurrected by `db175cca` had already been removed again by
`5f7d6f73` before this audit. Each is under
`hirsel-pipeline-tests-test-front::modes_explanations::`; their original removal
is cited individually below. `6050ca60` repaired the physical-path snapshots.

| Test suffix | Earlier removing commit | Latest cleanup |
| --- | --- | --- |
| `case_1_borrowed_parameter_into_mut_storage` | `a213f336` | `5f7d6f73` |
| `case_2_mut_parameter_as_global_result` | `a213f336` | `5f7d6f73` |
| `case_3_global_store_and_global_result` | `a213f336` | `5f7d6f73` |
| `case_4_task_of_another_scope` | `a213f336` | `5f7d6f73` |
| `case_5_erased_brand_into_branded_binding` | `a213f336` | `5f7d6f73` |
| `case_6_parameter_in_the_wrong_arena` | `a213f336` | `5f7d6f73` |
| `case_7_escaping_capture_of_a_borrowed_parameter` | `a213f336` | `5f7d6f73` |
| `case_8_result_of_a_temporary_closure_environment` | `a213f336` | `5f7d6f73` |
| `constructed_record_placed_by_its_part` | `a213f336` | `5f7d6f73` |

The other known union restorations are also absent: the Free full-pool waiter
fixture removed in `13f723de`, and `MEM-14-dynamic-list-replace-{run,fail}.hir`
removed in `e635d136`, were cleaned up again in `5ca29cdb`.
The audit changes no failure classifications and claims no cleanup already
performed by those commits. The focused Buck2 check
`buck2 test //:hirsel-pipeline-tests-test-front-shard-1 -- --test-arg=modes_explanations::`
ran 11 tests: all nine previously hidden tests and two related controls passed.
Focused tool checks passed 6 registration tests, 6 merge tests and 17 landing
tests (29 total).
