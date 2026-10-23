# Textindex friction

External evidence: `/workspace/code/hirsel-briefs/text-index-30/`. Original
failed drafts, diagnostics and immutable input seals are kept there, never in
application fixtures. Base main: `d49af62dc69a2237e2ebd35550638f68cc3abb46`.

| Minimal attempted source / witness | Actual diagnostic or behavior | Expected contract / workaround and cost | Disposition / owner / active effort |
| --- | --- | --- | --- |
| `let id = text(...)?; set.insert(mut seen, unique id); Document(id: take id, ...)` (`draft-1`) | OWN-2: `id` was moved by `unique id` | Intended: `unique` consumes an already unique value. Use `copy id` for the duplicate-ID set. One counted share; no extra string allocation. | Intended / text-index-30 / ~2 min |
| `fn span(...); for span in ...` and `fn posting(...); for posting in ...` (`draft-1`) | LANG-4: local cannot shadow a top-level value | Rename decode helpers `decode_span`, `decode_posting`, etc. No runtime cost. | Intended / text-index-30 / ~2 min |
| `store.validate` original nested term-character scalar loop (`draft-2`, `validate-alone.hir`) | TIR `CleanupMismatch`, body0/block90/instruction14/cleanup99: expected locals16,25,118,187,188; actual187,118,25,16. Standalone export witness uses block94/locals190,191. | Soundness-relevant missing cleanup, not an intended source rejection. Move scalar-character validation into a pure helper; no extra copy/allocation, same checks. Original app and reductions routed to coordinator. `check` alone on standalone reductions does not lower; `export --tir` reproduces. | Open producer defect owned by cleanup-order-30; application scope workaround / ~12 min |
| Generic `append` deep-copied its prior bucket; a scoped `map.update_value_with` callback tried `list.push(mut items, unique value)` (`draft-3`) | MODE-1: a generic borrowed capture remains local; a strengthened unique map then required unique key/value contracts on hand-back | Use `map.remove` to transfer the owned bucket, append the taken unique value, and reinsert; annotate `@preserves_unique(groups)` and unique receiving key/value rows. No deep bucket copy, but map removal shifts dense entries. Two real instantiations: Span and Posting. | Intended ownership rule / text-index-30 / ~6 min |
| `native::Fixture { file_reads, .. }` with actual main requiring Args and two mounts | Fixture has no Args/mounts/initial-files fields; `launch_fixture` and `launch_live` are crate-private, public `launch_runnable` uses default Fixture and empties its root. Standalone live grants intentionally do not apply harness file-read gates (`launch.rs:2029`, `sim_file_pending_29_never_changes_host_file_reads`). | Full actual main Pending cancellation uses current `SimulationConfig.file_reads`; no surrogate entry or new adapter. Public native scripted/Pending matrix remains INCOMPLETE. Exact current API witness: external `native-plan.md`. Coordinator confirmed this boundary and requested a follow-up, not API widening. | INCOMPLETE / coordinator @agentlang-24 / ~10 min |
| First background check shell ended before EXIT marker, while its resident compiler finished | Original log has diagnostics but no unconditional completion receipt | Retain as setup/incomplete receipt; subsequent commands keep the producer session and disable resident compiler. No semantic pass claimed. | Tooling / text-index-30 / ~2 min |

Streaming schema-order decoding, explicit error tags, lexical path validation,
byte reservations, task result staging, Shared document tables, and atomic
publication are ordinary application policy. They are not language defects.

Engine and replay receipts below are actual executions. Source checks, export,
requested width and zero-test wrapper success are never engine passes.

First engine results: the 256-byte token's build completed in reference
(983761/1000000 steps), but its inspect at `std@1/unicode.hir:7:39` stopped at
1000000 with no observation. The original 128×dotted-I token build also stopped
at 1000000 (`std@1/string_case.hir:19:16`). These original cells remain
INCOMPLETE; no bound was raised and no smaller vector substituted. Smaller
Unicode success vectors already passed independently. The harness reports Steps
and skips dependent stages, never increments semantic completed counts for them.

The original first failing test scratch used TempDir cleanup, so completed
observations preceding its Steps stop were lost; exact vector/source seals and
usage/pass identities remain in immutable command logs. The harness now retains
evidence even on failure and writes each initial/resulting world before advancing.
Any resumption names its persisted world and engine/stage selection; earlier
greens are not rerun to regenerate evidence. This is a harness evidence limitation,
not an application or language bug (~4 min, application owner).

Native full-main scripted/Pending fixture follow-up now belongs to
`native-harness-gaps-30` (coordinator disposition 2026-10-23). Neither generic
Fixture breadth nor a new runtime adapter is part of this application lane.


## Retained disposition at application handoff

The focused CLI harness adds four test identities (CLI inventory 436 -> 440;
text_index_30 0 -> 4). Original setup rejection and two original Steps-panic
logs remain external; affected/unstarted selections subsequently ran at exactly
the original fixture budgets. A test asserting a tooling stop can pass while
its semantic obligation remains INCOMPLETE. Per-cell receipts override wrapper
summaries. The first boundary wrapper mistakenly printed INCOMPLETE 0 although
16 per-cell Steps markers were correct; its counter is fixed, without repeating
those cells (~1 min, harness owner).

| Boundary | Actual result |
| --- | --- |
| Reference / checked H full main | 99 completed stage checks each; full observations compared where both were available. 16 Steps stops each, no observation. 12 dependent stages each UNRUN after build Stops. |
| LLVM / Cranelift production host files | 136 application launches each, all expected stdout/stderr/exit and independent Python oracle checks pass. Setup builds/reused builds are counted separately. |
| Physical placement diagnostics | Every native launch retained running/effective=N/pending=0. Representative success and domain error used requested 1/2/4 with logical jobs=4 and a fixed four-CPU wrapper allowance. This proves installed capacity, not simultaneous CPU utilization. |
| Immutable host tapes | 32 tapes, each replayed by reference, checked H, LLVM and Cranelift: 128 checks, complete output/error/exit and consumption; original provider paths unavailable. Only admission timestamp and placement were normalized across native widths, with complete ordered events preserved. |
| Replay refusal | Changed event, unconsumed event and changed canonical config refused in all four engines; changed source refused in all four; incompatible replay/grants argv refused in both native launchers. Original tapes unchanged. |
| Pending cancellation | Actual main reference + checked H pass with a genuinely parked file read; no later batch admitted. Native scripted main, native Pending cancellation/cleanup and its four-engine cancellation replay remain INCOMPLETE, owner native-harness-gaps-30. |

The sixteen Steps identities per reference engine are: token-cap inspect/all/any;
normalization-expansion build/inspect/all/any; normal-term-cap inspect; bytes-cap,
total-cap, total-over, terms-cap, terms-over, postings-cap, postings-over builds;
and index-cap inspect. Small fixtures remain at 1M; declared boundary fixtures
remain at 8M. bytes-cap/total-cap/terms-cap/postings-cap dependent inspect/all/any
stages remain UNRUN (24 stage/engine cells). No smaller vector or post-failure
budget increase substitutes for them. Native versions of these original vectors
pass. Pending full-main replay additionally depends on the native fixture gap.

Native selection initially wrote the unfiltered case corpus although the Rust
selector was narrowed: all 99 executed small native workflows per backend were
retained, not silently credited to the requested subset or rerun. The selector
now materializes its selected JSON before consuming it (~2 min, harness owner).

Source and evaluated build-input/stamp/config seals were taken before consumers
and checked at boundaries; detailed toolchain-byte seals precede native builds.
Some early evidence is explicitly incomplete: the first shell had no EXIT
marker; first-panic TempDir cleanup lost preceding raw observations; one
post-run boundary seal was taken after two test-helper edits and lists them.
An earlier live boundary check retained zero changes. These evidence defects do
not justify rerunning completed greens. Later fixtures retain initial/resulting
worlds even on failure. Missing large reload cells used already persisted native
index bytes with explicit external provenance, never repeated green builds.

The six handwritten application modules total 1022 lines: main 84, manifest 188,
tokenize 90, index 217, store 311, query 132 (tests/fixtures/notices excluded).
No compiler, runtime, ABI or std changes are included in this application.
