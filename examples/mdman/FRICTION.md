# mdman friction and verification

External receipts and failed drafts live at `/workspace/code/hirsel-briefs/man-render-30/`.
That directory is archival, never a runtime dependency. Active minutes are approximate.

| Attempt | Actual diagnostic or behavior / expected contract | Workaround and allocation cost | Minutes | Disposition / owner / witness |
| --- | --- | --- | ---: | --- |
| Enforce AST limits before `markdown.parse` | `parse_bounded` now admits original UTF-8 bytes, cumulative parser records and nesting before construction. | Keep16KiB source, add131072 parser work/128 nesting, then validate final depth32 (root one)/nodes8192; output caps and render-admission/worker-return copies remain. Counters are not RSS or final AST counts. | 4 | Addressed by markdown-parse-budget-39; original cap/tape acceptance receipts below remain historical, new refusals are reported separately |
| Reserved `where`, module-head `out`, shadowed `argument`, foreign record pattern, unblocked match assignment, sort callback without `@noalloc` | LANG-14/LANG-4/parser/MEM-5 rejections (check01–05). | Rename, owner helper and `replace` for foreign fields, block match arm, annotate pure comparator. No additional data copies. | 8 | Intended authoring contracts; draft01–05 and check logs |
| Send parser result as unique | `parse_bounded_owned` now carries deep uniqueness from the shared literal/general builders. | document.load borrows for validation then takes the tree into Parsed, removing its completed-AST copy. Coordinator render-admission AST and worker-return output copies remain. Payload construction copies remain. | 2 | Addressed by markdown-owned-ast-40; no ownership relaxation |
| Consume qualified results inside carried batching loop | Source accepted; V rejects ListConsume_open call result at body1/block21. Four-module witness reproduces without Markdown/std. | document.append receives owners outside coordinator loop; no additional copy/allocation. | 16 | OPEN; while-consume-mode-30 owns repair; witness-modules/batch-01.hir, check01/09, scripted02 |
| Diagnose through `file_definitions` | Rust E0624 private API,0 tests executed. | Used public lowering query to isolate error; temporary diagnostics removed. | 2 | Fixed setup; scripted01–03 |
| Early seals omitted unpacked compiler trees/DSOs | Seals01–10 were partial tool inventories; seal01 ran during setup. | Seal11 onward includes actual Rust/LLVM trees, ldd inputs, stamp reads/outputs/probes/globs. Native/checked artifacts materialized during harness build have explicitly late overlays. | 3 | Evidence limitation; seals and native-host-scope.md |
| Capture scripted per-cell results | scripted04 passed1 libtest test, but successful libtest capture suppressed per-ID stderr, including Steps dispositions. | Preserve aggregate; do not repeat accepted vectors to improve logging. New partitions use visible receipts. | 2 | INCOMPLETE per-ID provenance for original40 reference+checked fixtures; scripted04.log |
| Native scratch retained only after all replay assertions | LLVM47 live+47 own replay checks completed;16 tapes passed both host adapters, then an unnamed tape returned no observation at fixed10M and panic removed the scratch. | Keep scratch before execution. Preserve original failure/log and32 named adapter passes; no regeneration of accepted LLVM cells. | 4 | Fixed harness; lost LLVM tapes/control/input receipts remain INCOMPLETE; native-llvm-02-host.log and llvm-original-disposition.md |
| Host replay of larger immutable tapes | Cranelift source16384, total131072, nodes8192 and nodes8193 return no semantic observation at fixed10M in both host adapters. Public API returns no step_usage/outcome detail. | Retain tapes unchanged and exact no-observation disposition. Do not call it Steps or raise the bound. | 1 | INCOMPLETE8 host replay cells; native-cranelift-01-host.log |
| Production native scripted regular-file Pending/faults | CLI refuses `--simulation` with `--native`; native Fixture readiness applies to Test trees, not root-Io Args/files/mount SimulationConfig. Host regular reads ignore Test readiness scripts. | Full real filesystem native runs; actual Pending cancellation in reference+checked H. No surrogate entry or new runtime adapter. | 3 | INCOMPLETE native cancellation/readiness and scripted cross-width event matrix; native-pending-adapter-01.log, pending-world.json; @agentlang-24 |
| Physical width evidence | Socket visibility alone is not readiness. | Retain running/effectiveN/pending0 control replies on all47 Cranelift cells; requested widths1/2/4 use fixed logical jobs4 and a four-CPU wrapper. No simultaneous-CPU claim. | 2 | Qualified capacity for retained Cranelift cells; cranelift-control-summary.json. LLVM original raw receipts lost as above |
| New native seeded fixture template | Explicit `inputs` lacked required hash/random/test seed fields; launcher refused before entry. | Add explicit zero seed snapshots, rerun only new failed/unrun fixtures. No app change. | 1 | Fixed setup; native-llvm-additional01 |
| Initial config refusal vector | Serializing changed config via generic JSON changed canonical field order; refusal was noncanonical-header validation. | Preserve initial4 format-refusal receipts; replace only this control with byte-preserving canonical Args change and run its4 affected engine cells. | 2 | Harness control refinement; controls01 and controls-canonical01 |

The original man-render-30 app was923 Hirsel lines: main51, manifest312, document104, roff268, batch115,
publish73. That original lane changed no existing application or std source. Five focused Rust test
identities are registered through exactly one `modules.rs` declaration. The initial
harness listed439 identities (436 prior plus the initial3); adding2 focused test
identities yields441 on this base, independently of later main registrations.

Verification is deliberately not an all-green engine/replay claim:

- scripted04:1 aggregate PASS covering40 fixtures × reference/checked H, with
  unrecoverable per-ID observation/Steps accounting as described above.
- additional01:6/6 observations PASS (Pending cancellation and queued-admission
  suppression/cleanup; seeded batch; failed commit followed by another publication).
  Fixed1M bounds consumed149762,180607 and96563 steps respectively in both engines.
- LLVM original:47 native live and47 native replay assertions completed;32 named
  host-adapter passes. Later replay assertion failed; raw originals lost.
- Cranelift:47 live and47 own immutable replay PASS, all47 settled control receipts;
 43 tapes ×2 host adapters PASS,4 tapes ×2 no-observation INCOMPLETE at10M.
- Retained Cranelift tapes:47/47 LLVM cross replay PASS with originals unavailable,
  exact stdout/status and immutable tape hashes (cross-replay01).
- controls01:16/16 source/input/extra/config-format refusal checks PASS across the
  four engines. Canonical-config refinement passes4/4 at unchanged bounds (controls-canonical01): canonical changed Args refuses recording.run_identity binding.

- metadata01 adds8/8 reference+checked-H observations for source/manual label64/65
  and an explicit supplied date/section9 wrapper. These are new cells, not repeats.

The five new metadata/seeded workflows additionally pass LLVM5 and Cranelift5
live cells, their10 own-native replays,20 host-adapter replays and10 cross-native
replays (native-*-additional and cross-additional01). Those new tapes are retained.
Their host replay assertions compare complete stdout/status with the recorded run.
Native scripted fault/pending cells are printed as INCOMPLETE by the harness;
they are excluded from selected/pass counts, not silently credited.

The final support inventory contains47 source workflows. Original40 scripted
per-ID provenance remains unavailable. 8 larger
host replay cells, lost original LLVM artifacts, production-native cancellation
at widths1/2/4 and scripted event equality across widths remain INCOMPLETE.
Live host metadata is compared only with its own tape; timestamps/task IDs/events
are never sorted or discarded to fabricate cross-width event equality.

Native child runs use tools/capped, CPU60/RSS2GiB and fixed HIRSEL_JOBS4; native
builds are separate setup. The Rust harness performs compilation and several host
adapter executions in one process under Buck's ordinary test guard; its aggregate
CPU/RSS policy is not a per-fixture native-cap receipt. Tool/hash seals and late
supplements are labeled explicitly; no unchanged greens are rerun after main moves.

Reference is cmark-gfm27d942c8 src/man.c:20–274 and api_test/main.c:569–611;
LICENSE.cmark preserves its BSD license. The independent pinned binary matched5
literal no-wrap body vectors; empty source and nested fonts differ intentionally.
Other deviations: deterministic `.TH`, no wrapping option, HTML/table/unsupported
node rejection, control sanitation, bounded output, owner-safe traversal and
all-validated-before-publication batching. Failed file publications are counted;
no multi-file atomicity or crash durability is claimed.

Parser admission (markdown-parse-budget-39) qualifies the historical results above:
the original `depth32` source used a depth-zero document root and now refuses
`AST depth` with root depth one at the unchanged32 cap. Its bytes and the original
receipts remain unchanged; current expected refusal is explicit in the fixture.
Parser work131072/nesting128 supplement final node/depth checks, and do not solve the archived replay limitations.
Source coordinates are now supplied by the bounded immutable ParsedDocument;
validation drops its views before consuming into_node for rendering. The additive owned parser
APIs address the producer ownership gap without changing these admission limits.
