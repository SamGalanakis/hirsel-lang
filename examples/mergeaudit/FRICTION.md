# Implementation record

Base: 9f52bf1c; original schema, cap vectors and independent oracle outputs were
frozen under hirsel-briefs/merge-audit-37 before app implementation. No language,
std, compiler, native runtime or worker topology changes belong to this lane.

| Original / expected rule | Actual response / adaptation | Cost | Time | Disposition / owner |
|---|---|---|---|---|
| node-diff3 mutable arrays, shift and stable sort; overlap includes touching endpoints | Ported bounded indexed Lists; explicit base/side/ordinal order; retain deletion coverage and all three spans | Linear metadata only; O(h log h) std sort; no text copies in graph | 20 min | intended / merge-audit-37 |
| Upstream LCS may choose another repeated-line alignment | Use std MyersMinimal with its sliding; independent DP verifies minimality/reconstruction, common-hunk port verifies sweep | Separate oracle seam and fixtures; no reimplementation of diff in app | 15 min | intended / merge-audit-37 |
| Borrowed line intervals must not escape immutable owners | Generic View[T] returns in items; plans own source triples, materialize only for reports/output | Sources retain split line owners and digests; std split_lines copies tokens, original text is dropped | 10 min | intended / merge-audit-37 |
| Tasks require owned sendable captures | Move each triple into one child, return owned Result; ordered awaits join before publication | One closure/result per file; no shared mutable graph | 5 min | intended / merge-audit-37 |
| Initial claim that 129 conflicts was unreachable under 256 lines was incorrect | Preserve frozen original helper vectors; add a full-main one-over witness with 256 base lines, 193 lines per side: 128 alternating delete/modify pairs plus a leading insertion; the resolved-text cap alone is unreachable; escaped reports can reach it | Additional full-main original-cap action, no smaller substitute; correction frozen before this witness is consumed | 5 min | corrected analysis / merge-audit-37 |
| Manifest JSON needs finite preparse admission too | Add 16KiB manifest, 128-byte leaf name bounds; all requested caps unchanged | Limits manifest spelling, no extra codec | 5 min | intended / merge-audit-37 |
| Remote evidence must survive ephemeral roots | Embed reviewed tools/capped bytes; retain returned archive and source/config/tool seals using app-test precedent | Lane-only copies of archive/seal/wrapper helpers; tool digests retained | 5 min | intended / merge-audit-37 |

| Local `result` bindings | LANG-14: `result` is reserved for the returned value in a postcondition; renamed locals to `values` | Naming only, no copies or weakened checks | 2 min | intended / merge-audit-37 |

| Assignments as unbraced match expressions; locals `interruptible`, `kind`, `binding`, `region` | Parser requires statement blocks; LANG-4 forbids top-level and module-name shadowing; use blocks and distinct names | Naming/syntax only | 4 min | intended / merge-audit-37 |
| `Result[View[T], Problem] in items` followed by `?` inside a global-result helper | MODE-1 correctly relates the whole Result, including errors, to items; split global `check_span` from nonfallible borrowed `view` | One plain validation helper; zero line copies, same bounds | 5 min | intended / merge-audit-37 |
| Task captures/results with unqualified counted payloads | CLO-3/MODE-4 require nested ownership: unique List[unique Triple], unique Result, explicitly owned options | Deep unique Plan result at child boundary (one bounded copy of source line owners and metadata); one small Options copy for interruptible mode | 8 min | intended / merge-audit-37 |
| Local CLI check without toolchain std image | Original check stopped at 60 seconds, 637 MiB; retain log; subsequent check binds actual Buck std image, same app and caps | Tool input selection, no budget increase; source check is not execution | 2 min | open original identity / merge-audit-37 |
| Early dev build began while v2 seal was finishing | Retain partial timing and both logs; no acceptance claim; v3+ deltas bind completed v2 tracked/tool seal before subsequent source consumers | Additional closure overlays and retained CLI/image bytes | 2 min | partial dev evidence / merge-audit-37 |

| Native full-main build on 9f52bf1c | `NativeError { stage: Manifest, message: "unreviewed native declaration digest: arena.bounded" }` at build before any backend execution; consumed only main-reds-37 owner repair 0ecb7dcc after readers exited | Zero tapes/executions for original native-v12; first 50 interpreter observations retained, never repeated | 5 min | fixed upstream repair consumed / main-reds-37 |
| Borrowed manual decision variant at a global output boundary | Whole Result locality again applies to `?`; choice now returns a unique typed Choice | Manual text copied for owned selection, then copied into the final output builder; enum-only choices are plain | 3 min | intended / merge-audit-37 |
| JSON boundary ownership | `unique` copies extract manifest/decision fields from parsed JSON; report fields copy names/digests, content spans materialize exact lines; std diff token IDs own their keys | Bounded by 16KiB envelopes, 128KiB source and 256-line owners; no custom codec or whitespace normalization | implementation | intended / merge-audit-37 |

| Shared width fixture helper has no caller-supplied runtime wrapper | Lane-local copy of width_capacity_36::fixture_capacity preserves public APIs and exact running/requested/effective/pending assertions; add the same tools/capped wrapper with HIRSEL_JOBS=4 and 60 CPU-s / 2GiB limits | About 170 Rust helper lines; no change to the other lane or fake topology | 10 min | intended / merge-audit-37 |
| Objects are linker inputs produced after the source/tool seal | Retain exact emitted ObjectArtifact bytes and hashes before linking; archive each object in its own bounded frame, ordinary tapes/reports first | Lane-only evidence helper; original 4MiB frame bound unchanged; source/decisions/runtime caps unchanged | 5 min | intended / merge-audit-37 |

| Original `fields` refusal fixture has an extra root key, valid choices array and binding | Actual stderr `mergeaudit: choices`, expected frozen `mergeaudit: fields`: model.decisions combined envelope arity and choice-count checks. Split them; extra envelope keys report fields, count overflow still choices. Original input and expected stderr preserved | Three Hirsel guard lines; no relaxed schema or substituted expected text. Recover only fields and later unrun refusals; first 14 PASS observations retained | 5 min | fixed / merge-audit-37 |
| Native four-file and original cap workflows exhaust Steps1M | Native-plan SHA170:28; upstream-regions JSON405:25; source/lines/total/conflict exact and one-over SHA sites; decisions-exact JSON29:5. All are cumulative workflow stops, not isolated library-cause claims | No cap/input reduction. Native-plan build/object emission is setup only, zero live/tapes. Original pre-reader generators and late explicit fixture reconstructions are distinguished | qualification | open / app-budget-reds-35 (SHA), app-steps-37 (JSON) |
| Aggregate tests have completed prefixes before a later failure | Add explicit case/refusal recovery selectors, assert nonempty selection; capture actual fixture JSON before each new run | Test-only helpers, no app behavior change; no accepted-prefix rerun for improved evidence | 5 min | intended / merge-audit-37 |

| Initial output cap covered selected text but omitted escaped reports | Escaped NUL text can produce >256KiB while source/line bounds hold. Freeze two additional originals before fix: identical14553-byte sources,43659 total, one line each; names aa/aaa produce exactly262144/262145 report bytes. Preflight metadata shells and quoted content via std.json, admit every sum before final report encoding/publication | One extra encoding of bounded metadata and each <=32KiB scalar (temporary <=196610 bytes), plus bounded metadata-map copies; no second escaping codec, no input/cap reduction. Resolved-text limit remains independently checked | 15 min | fixed app admission; original qualification separate / merge-audit-37 |
| v20 width and refusal receipts |18 live+18 own native replays at exact requested/effective widths 1/2/4, running and pending0 on both backends; full events, initial tree, Args and raw streams compare byte-identically across widths/backends. All14 remaining refusal observations pass, original four-files reference still SHA Steps1M | Preserve all prior greens; report-render changes affect plan predicates, not resolve/early-Pending width paths | qualification | fixed predicates retained; four-files open / app-budget-reds-35 |
| Candidate owner SHA repair | Consume landed47c52687 primitive dependency and887aad8e SHA/path slice only after v20 readers exit; no unlanded eight-round experiment | Upstream commits retained separately; no claimed recovery until missing/affected original cells execute | 3 min | owner repair consumed / app-budget-reds-35 |
| public differential HeapWorld omits fixture file-fault queues | Commit fault uses SimulationConfig::from for reference/checked H and public native Fixture for both backends; omit only the extra differential_in Heap row for that fault recipe | No lost required engine path, no private adapter or duplicate fixture codec | implementation | intended / merge-audit-37 |

| Metadata shell measurement reuses head/file_fields after `unique value` | OWN-2 at report.hir:84/146 and69/149: a currently unique binding is moved by the unique expression. Use `unique copy` for the three metadata-only maps, preserving the originals. This is intended ownership, not a compiler disagreement | Explicit bounded metadata copies already accounted for; no schema, oracle, input or cap change. v22 setup refusal retained | 3 min | fixed source / merge-audit-37 |

| Native exact-report byte oracle sorted object keys with serde_json::Value | Actual LLVM main exited0 and wrote262144 bytes; original expected also262144 and parsed equal. First difference is key order: std Map/JSON preserve explicit insertion order (schema,binding,files; file name,regions; region base,left,right,kind,id,texts). Keep both original byte files and frozen limits, change only the independent fixture layout, still serde-escape every string | Test-only oracle correction. Reuse the retained successful LLVM tape for missing commit/payload assertions and cross-native replays; no repeated live run for evidence | 5 min | oracle corrected, remaining predicates separate / merge-audit-37 |

No qualification claim is made by source checking alone. Execution receipts,
original failures/stops and any owner-routed witnesses are retained outside the
repo; the final report states each incomplete identity.

## Retained qualification obligations

Original test names and full default selections are preserved. The independent
refusal subset has its own `merge_audit_37_refusals_only` name; it does not recover
the original `merge_audit_37_refusals_and_independent_files` aggregate. Ignored
originals retain every fixture byte, expected result, and Steps1M assertion.
Run them explicitly with the named filter and `--ignored` after an owner repair.
No ignored result is counted as a pass. Prior SHA/JSON repair lanes are settled;
remaining work is escalated to **@agentlang-24** for assignment, with the exact
external source/input/tool/image/cap receipts and historical owners retained.

| Original identity | Remaining predicate / original witness | Follow-on |
|---|---|---|
| `refusals_and_independent_files` | Final four-files plan: four identical independent\n source triples, reference Steps1M at SHA435:5; checked H unrun. Fourteen preceding refusal cases separately passed both interpreters | @agentlang-24 (prior SHA owner app-budget-reds-35) |
| `native_full_main_replay` | Four anchor/old/left/right triples, full plan, published-plan-fed resolve, domain, denial, Pending cancel and commit; reference plan stops Steps1M at diff38:17 after SHA repair, before any native launch. Compiled objects are setup evidence only for this original aggregate | @agentlang-24 (diff/cumulative workflow budget) |
| `cap_source_exact` | Original32768-byte source, reference Steps1M SHA355:5; checked H unrun | @agentlang-24 |
| `cap_lines_exact` | Original256-line source, reference Steps1M SHA361:73; checked H unrun | @agentlang-24 |
| `cap_total_exact`, `cap_total_over` | Original131072/131073 total-byte worlds, reference Steps1M SHA318:16; checked H unrun | @agentlang-24 |
| `cap_conflicts_exact`, `cap_conflicts_over` | Original128/129 conflict worlds, reference Steps1M SHA354:9 /371:9; checked H unrun | @agentlang-24 |
| `cap_decisions_exact` | Original16384-byte decisions, reference Steps1M JSON28:15; checked H unrun | @agentlang-24 (prior JSON owner app-steps-37) |
| `cap_report_exact`, `cap_report_over` | Original262144/262145-byte reports from14553-byte NUL-bearing sources; reference Steps1M SHA355:73 /364:5; checked H unrun. Separate native boundary action passes all four backend/cap cells and eight cross-native replays | @agentlang-24 |
| Host replay and adversarial controls on four-file plan/resolve/commit | Native-to-reference/checked-H replay and binding/request/missing/extra host controls remain gated in the original aggregate. Native-only recovery is independently named and cannot recover those predicates | @agentlang-24 |

Adaptations preserve std Myers alignment rather than upstream LCS tie choice,
LF/CRLF bytes rather than whitespace tokenization, bounded indexed ownership
rather than mutable shift arrays, and explicit ordinal tie order. Those are
semantic choices, not unresolved language defects. No throughput claim is made.

| Host replay accounting | v27 reference replay matched the denial observation, then the test incorrectly unwrapped final_state. Public host replay is semantic enrichment with exact bound request consumption; only simulation retains a snapshot. Bind original complete file maps, read bytes, atomic handles, successful/refused commits and raw stderr to retained Tree/events via an independent accounting oracle | Test-only protocol oracle; no app changes or removed publication assertion. Reuse both accepted plan tapes and LLVM denial tape; no repeated live cells | 8 min | fixed test contract / merge-audit-37 |

## Accepted evidence at the landing boundary

The lane adds 24 named Rust tests (12 default checks,11 explicitly incomplete
originals and one explicitly selected capacity harness). This is an inventory,
not a claim that a single 24-test action passed. Before: zero mergeaudit tests;
the two existing affected pure-model controls both passed after the change.

All 15 fixed vectors have plan plus four explicit resolutions on reference and
checked H: 150 accepted full-main observations across the retained generations.
The changed report path separately recovered 28 plan observations plus the
original upstream vector's 10 observations. Fourteen refusal cases contribute 28
observations, and five original one-over caps contribute 10. The independent
oracle passes 358 scenarios, 716 DP checks and 358 common-hunk sweeps.

Ordered capacity passes 18 live + 18 own replay cells (both native backends,
widths 1/2/4, success/domain/cancel), with actual requested=effective, running and
pending0 controls, byte-identical full scripted events/raw streams, delivered
Interrupt/Blocked read, and exact publication audits. Report boundaries pass 4
native cells and8 cross-native replays (one retained successful LLVM live reused).
Missing native plan/denial/commit recovery passes 6 live identities,12 cross-native
replays and8 native adversary controls. Denial also passes 2 live interpreter
observations and4 native-tape host replays; previously completed LLVM predicates
were reused. Two retained plan tapes pass the missing complete file/handle/state
accounting predicates without executing the app again.

These independently named receipts do not recover the ignored original
four-file reference aggregate, its host replay/control obligations, or any of
the nine original cap/reference Stops. No source/input reduction, Steps increase,
completed accepted-cell repetition or four-engine-complete claim is made.

The shipped manifest and decisions were also exercised with the actual CLI:
plan then resolve both exit0, and merged.txt equals the exact shipped left bytes.
This supplemental smoke used tools/capped's default4GiB ceiling (observed peaks
193/136MiB), so it is not credited as a 2GiB native acceptance cell. All native
qualification uses the explicit60CPU-second/2GiB wrapper retained with each tape.
The `--list` action confirms24named tests and is inventory only, not24executions.
