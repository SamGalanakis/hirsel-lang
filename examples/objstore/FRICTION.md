# Object store authoring and qualification

Base d49af62dc69a2237e2ebd35550638f68cc3abb46. Evidence and frozen matrix:
/workspace/code/hirsel-briefs/object-store-30/. Exact original receipts remain
separate from later application changes and missing-cell continuations.

1. Draft01 check rejected `digest`/`codec`/`child` module-head locals and a
   `root` parameter shadowing store.root (LANG-4); renamed them. `arg.text`
   yields shared Strings, so CLI argument copies are explicitly unique.
   Generic load inference at the unique return expected a nested unique
   instantiation; an intermediate binding preserves the codec's actual type.
   A borrowed previous-entry name uses a declared borrow cursor instead of an
   owned copy. The optional signal worker explicitly takes its <=1KiB names.
   Intended rules, application owner; ~10 active minutes, no language repair.
   Original exact source/diagnostics: external draft-01/ and check-01.log.
2. Setup prepare01/02 detached producers did not complete or write an EXIT
   marker; these are unexecuted setup, no test credit. prepare03 uses a retained
   exec session and completed EXIT0. ~3 minutes, tooling/application owner.
3. Draft02 `string.split(copy name, "/")` requires a global taken String
   (MODE-1); copying a borrowed String retains its locality. key now scans
   bounded borrowed bytes, avoiding an extra <=1KiB deep copy and split list.
   Intended ownership contract, ~2 minutes, fixed in application.
4. First full-app matrix executed 84 passing reference/checked-H cells and
   stopped at parent-symlink/reference: oracle expected path `d`, app reported
   the full requested `d/a`. Preserve test-01.log and original oracle; correct
   the diagnostic oracle and run only the six remaining identities. No app
   behavior changed, ~2 minutes. Selected harness1, failed1, workflows84 pass.
5. Native01: four selected identities x two backends completed live semantics
   and both native replays. First reference+checked host replay agreed, then
   the harness unwrapped final_state=None: host observations intentionally
   expose effects, not a simulated filesystem snapshot. Compare recorded stderr
   writes and complete observation equality instead. The TempDir panic cleanup
   deleted the native01 tapes/receipts (a harness evidence defect); original
   failure log stays native-01.log. These eight original live cells and their
   absent-tape obligations remain INCOMPLETE for archival/replay evidence;
   no rerun for retention. Fresh unrun identities keep directories before
   execution. Tool/image hashes were pre-consumer, new executable hashes were
   missing before that first launch and are not retroactively claimed. ~8 min.
6. Application review found a real cancellation/publication race: a signal
   select could win after the build worker published a root. Move publication
   after select has canceled/joined its loser; no cancellation result advertises
   a root. Preserve pre-publication-fix/. Snapshot-dependent cells are affected
   and revalidated; unrelated put/get/list/verify greens are retained. ~8 min.
   Manifest state now accounts for input plus admitted row metadata jointly
   before growing the index. Application correctness fixes, no compiler change.
7. Native02 completed18 live +36 native replay cells and10 reference/checked
   host replays before llvm-verify/reference returned `replay produced no
   semantic observation` at its original1M bound. The same verify input hit
   Steps in both original simulated engines. Preserve that unchanged bound and
   attempt the remaining checked counterpart and unstarted tapes separately;
   no larger budget or smaller substitute. Owner application tooling policy.
8. Exact accepted blob16KiB, total128KiB and objects128 fixtures reached their
   original40M step bounds in both reference engines; retain all six tooling
   stops. Files32, depth8, name1024, manifest256KiB and entries33 completed in
   both engines. The large manifest used37,869,592 steps, under its frozen40M
   policy. No bounds changed and no smaller replacement is credited.
9. Production qualification completed18 outcomes (success/domain/cancel x
   LLVM/Cranelift x widths1/2/4), all with running/effectiveN/pending0 control
   replies before completion. Every child joined; cancellation advertised no
   root and leftover objects independently hash correctly. All36 native
   replays pass with original grants/directories removed. First qualified
   success reference replay at40M had no completed observation; the first CLI
   witness instead stopped when its resident compiler exited; this is separate
   tooling evidence and does not prove Steps. These larger host replay cells remain
   INCOMPLETE individually, not native semantic failures. Scripts/argv/grants/tape/tool
   hashes and metrics are external qual-*/. Capacity is not simultaneous CPU use.
10. Public main-entry native SimulationConfig/Pending is unavailable: current
   CLI refuses `run --native --simulation ...` before entry. Exact witness is
   pending-native-witness.log, while actual main Pending sibling cancellation
   and signal cancellation run in both simulated engines. No Test helper or
   injected Canceled result substitutes for the required native Pending cells.
   Owner native-harness-gaps-30 via agentlang-24; no adapter added.

11. Final input review bounded CLI UTF8 arguments before the explicit unique
    copy (previous draft copied an overlong argument before path validation).
    New argument1025 fixture refuses before granting objects or allocating a
    second large String. Existing <=1KiB inputs take the same effect-free
    length branch; their recorded revisions and complete receipts remain
    immutable, not relabelled as the new source. ~3 minutes, app owner/fixed.

12. Cross-width audit compares complete CRC-validated Event payloads in tape
    order, without sorting or removing IDs/errors/clocks. All12 comparisons
    first differ at event7, Metadata(source/manifest): independently created
    files have different modified_unix_ns. Their granted host paths also differ.
    These live recordings are different input worlds, not deterministic native
    scripted equality evidence. Keep the12 mismatches and mark that obligation
    INCOMPLETE; no semantic field is normalized away. The native public Pending
    refusal above is the current exact provider witness. Owner application
    qualification/native-harness-gaps-30; ~5 minutes. The first read-only
    comparison draft used offset16 instead of the17-byte magic and rejected
    before decoding any event; failed draft/log and corrected comparison02
    remain external. No application or replay was rerun for that audit.
13. Final argument-bound change is covered by a new two-native/four-replay
    refusal fixture and representative accepted put/get/snapshot/list/verify
    launches on the matching final binaries. The previous width qualification
    receipts belong to their exact pre-argument-bound revision; its normal
    inputs have unchanged decisions, but those receipts are not relabelled as
    final-artifact physical qualification. Source hashes for all six final
    modules match the final native build copies. No algorithm changed after
    the publication fix.
14. Host replay observations expose no precise tooling-stop detail. For28
    original cells (verify4, three large caps12, qualification success12),
    run_observed host adapters returned only `replay produced no semantic
    observation`; cause is NOT established by that generic string. The
    separate resident-compiler CLI stop is a tooling result too. Owner
    native-harness-gaps-30 via agentlang-24; exact source/tape/tool hashes are
    retained. Four additional final-source verify replay cells have the same
    generic refusal at their unchanged1M policy. No bound was raised.

## Qualification receipts

Five registered CLI harness identities (436 ->441 total CLI tests); registration
is exactly one modules.rs declaration. Handwritten application:1023 lines:
main112, object130, ingest120, tree183, store150, snapshot328. Tests, fixtures,
licenses, documentation and generated build inputs are excluded.

| Complete actual app execution | Reference | Checked H | LLVM | Cranelift |
|---|---:|---:|---:|---:|
| Small scripted workflow identities |50 pass,1 Steps|50 pass,1 Steps|real-provider cells below|real-provider cells below|
| Cap/one-over identities, including argument bound |13 pass,3 Steps|13 pass,3 Steps|all accepted/refused correctly|all accepted/refused correctly|
| Production success/domain/cancel widths1/2/4 |separate tape replay|separate tape replay|9 pass,9 settled|9 pass,9 settled|

The scripted matrix has126 unique passing cells and8 exact original step stops;
12 affected publication-path rechecks also pass. Pending sibling cancellation
and signal cancellation run the actual configured main with public
SimulationConfig, not a helper. Each complete script observation has an exact
semantic-event replay and checks complete output, stderr, exit, files and
reference/checked-H equality. Retain the original failed parent-symlink oracle
assertion separately. A test wrapper's green with incomplete cells is not a
semantic pass for those cells.

Retained native receipts total158 successful recordings and316 LLVM/Cranelift
replays, covering functional, boundary, qualification and final argument paths.
Host reference/checked-H tape replay:284 pass and32 INCOMPLETE generic
no-observation outcomes, at unchanged bounds. Each immutable tape is replayed
with its original grants and mutable directories removed; the driver enforces
complete event consumption, output/error/exit and no real effects. Twenty
changed-input/config/checksum/missing-event/source-revision refusal controls
pass across all four adapters. Raw receipts are indexed externally, not shipped
as production data.

The eight native01 recordings and16 native replays lost by the initial harness
panic are disclosed separately and not included in retained totals; their
original archival/replay obligations remain INCOMPLETE. Later changed-source
publication cells have their own distinct receipts, never replacements for
those missing tapes. Earlier byte/tool seals and later supplements keep their
actual timing labels; no green was repeated for sealing or main movement.

**Required obligations remain INCOMPLETE:** eight observed reference/H Steps
cells,32 host replay cells without completed observations, native main-entry
Pending/scripted-file-provider coverage, deterministic cross-width equality,
the lost original tapes, and final-artifact physical-width requalification
following the small argument-bound guard. The useful app can land with these
explicit dispositions; this is not an all-engine/all-replay completion claim.
Free remains OFF. No runtime/compiler/std change or performance claim is included.
