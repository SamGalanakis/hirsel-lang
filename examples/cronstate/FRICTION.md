# cron-state-30 friction

Base d49af62dc69a2237e2ebd35550638f68cc3abb46. Source inventory: no cronstate
application or named Rust identities at base; three initial identities materialized.
External initial matrix, independently computed Go UTC oracle, immutable attempts,
input seals and selected/started/completed receipts are under
`/workspace/code/hirsel-briefs/cron-state-30/`.

| Attempt / actual diagnostic or behavior | Expected contract and adaptation | Cost / effort / disposition / owner / witness |
|---|---|---|
| prepare01 background shell left an empty log and no owned Buck client | Own-cwd prepare must complete and retain unconditional EXIT. Foreground tool session prepare02 exited0. sync01 originally failed missing Reindeer. | 2 min; setup, fixed; app lane; prepare01/02 and sync01 logs. Zero tests ran. |
| Rust `library::Fault`, `library::FileError` E0433 | Public types are in `library::simulation`. Correct imports; no adapter written. | 1 min; harness, fixed; app lane; attempt01/test01. Zero tests ran. |
| `Object({"event":Text(...)})`: LANG-1 expected newline/semicolon | Hirsel has no JSON/Map object literal. Use ordinary derived typed records and named hooks. | ~2 min; intended; app lane; attempt02/test02. One test selected/started but app not executed. |
| Missing `job_from_json`/`job_to_json` and `row_*`: LANG-4 | Derived nested nominal JSON needs explicit hooks in the nominal module. Add the four small derived hooks. | ~1 min; intended plumbing; app lane; attempt03/check03. |
| Outer local `fields`/`n` shadowed in a loop/pattern: LANG-4 | Local shadowing is forbidden. Use job_fields/row_fields/value_minute. | ~1 min; intended; app lane; attempt03/check03. |
| `unlock` returns `file.remove_dir(...) ? Files`: LANG-7 expected Result, found () | `?` extracts the unit payload; explicit Ok(()) returns the Result. | <1 min; intended; app lane; attempt03/check03. |
| `task.scope(... fn(){state.admit(mut book)})`: CLO-1 borrowing capture read-only | Mutable state must be owned in its orchestrating frame. Own a bounded book snapshot inside the session scope; pass its Scope down to batch. | One <=128KiB snapshot copy/session; ~4 min; intended; app lane; attempt03/04/check03. |
| Direct CLI check under60s stopped before diagnostic | Std source precheck can take about2min. Compiler setup is not an app semantic cell. Keep timeout as setup INCOMPLETE; use normal Buck harness setup limits next. | ~1 min; tooling; app lane; check04.log. No app execution. |
| First seals retained stamp without evaluating probes/globs; runtime artifacts not yet materialized | Later seals evaluate generated Reads against actual cwd, include every BUILD_INPUTS/output and actual local toolchain/CLI/image/archive bytes. Original seals remain honestly partial. | Hashing/tooling only; ~3 min; tooling fixed for later consumers; app lane; before-check04.json. |

Ordinary application costs: capped config DOM plus typed decode, bounded owned argv
snapshot per job, explicit JSON state snapshot publication and two sync barriers,
conservative uncertainty on interrupted completion publication. These implement the
capability/durability contract; no language defect or exactly-once guarantee is claimed.

Further exact dispositions:
- Full actual entry/module reference/checked matrix05:98/100 completed semantic
  passes. capacity-16/reference and checked-H both consumed the unchanged10M
  bound in std/json.hir:403:18. No observation; INCOMPLETE. No optimizer, bound
  increase or smaller replacement. Routed to @agentlang-24, message642983.
- Missing-only matrix08: pending config read actually parked, then canceled and
  returned run_until; uncertain-future persisted success for the future minute;
  exact24h coalesced1440 misses to1 with skipped1439. Both engines agree (six
  passes). backward-clock retains its original1M Steps in string.hir:30:59 on a
  virtual61-second session. Its source/vector/budget remain; INCOMPLETE, no rerun.
- LLVM native06: all9 live success/error/cancel width1/2/4 cells passed with
  effective=N,pending0,worker_phase=running held by a real child socket handshake.
  First CLI reference replay returned0 and exact app stdout but a compiler
  thread-creation EAGAIN warning on tool stderr broke wrapper assertion. Preserve
  the original setup red; use HIRSEL_JOBS1 for compiler-only replay independently
  of the fixed4CPU process wrapper. No live greens repeated. ~2min; harness/tooling.
- Cranelift native07: all9 live cells and R/H/Cranelift replays pass. Its wrapper
  also calls ordinary reference observation after CLI reference replay; these9
  repetitions are duplicate checks, not new distinct obligations. Additional
  LLVM replay of its tapes is a missing cross-backend partition.
- Native07 ended14:13:59+02, EXIT0, before reference08 started14:15:57+02. Source
  freeze comparisons find no tracked/owned changes. Newly materialized harness
  executables change across build boundaries and are retained as outputs. Earlier
  artifact seals missing actual pin trees are partial; later seals include the
  actual tools/buck2/toolchains files. No greens repeated to improve provenance.

- The extra-event refusal control incorrectly demanded empty stdout even though
  a missing/extra trailing event is found after the full recorded output. Native
  exit3 was correct. Failed control16 retained; control17 selects only failed or
  unstarted adapters and expects the exact recorded prefix for a late refusal.
  Four changed-input and two extra-event R/H controls were already green and
  were not rerun. ~2min; fixed harness, no engine defect.
- Tape audit14 failed Rust setup because hirsel_lir is not a CLI test dependency.
  Use the public hirsel_driver::native::TargetSpec re-export. Zero tests ran;
  audit15 passed all18 original ordered tapes. ~1min; intended host API boundary.
- The public native Fixture witness18 loads the actual manifest/main/modules,
  exports/imports native input and executes both backends with file_reads Pending.
  The current Fixture has no Args/mount/file input fields: its empty Args produce
  app usage exit2, not the required pending-read cancellation. Both actual
  Mismatch(exit2) receipts are retained. Native full-entry injected file faults
  and Pending remain INCOMPLETE; ordinary real files/processes supplement them.
  No adapter or helper surrogate is called their semantic pass.

- Final numeric native20 omitted trailing application argv after the grants
  operand. The standalone launcher replaced template Args with the explicit empty
  snapshot; app returned usage2 before reading config. This is a setup red, zero
  scheduled work. Source/assertion output retained in final-native20 log/draft;
  the temporary scratch was inadvertently dropped by the failed wrapper, so its
  tape cannot be replayed. Final21 keeps scratch from creation and supplies exact
  application argv. No successful cell was repeated. ~2min; fixed harness.
- Final application review found a past-UNTIL escape around the global deadline.
  Actual full main with config FileReadScript::Pending was Blocked at
  std/file.read_all_bounded (R/H,10448 steps), while the ordinary past stop read
  and rewrote state before returning0. Exact failed fixtures/source/effects are
  retained in past-witness24. Main now arms an already-expired scope at zero
  interval, so no read/admission can outlive the stop. A pending past fixture
  expects cancellation before it parks. This is an application bug, not a
  language defect. About5min active effort plus build queue; fix25 completed both ordinary/pending cases on reference and checked H (four passes,15495 steps each, unchanged1M bound).

- Six child-specific3s deadlines passed live, with all six running/effectiveN/
  pending0 handshakes retained. Each producer script then timed out at its first
  reference CLI replay (width1) under the original15s/10M caps. Master EXIT0 was
  only the last seal, not aggregate semantic success. Both raw timeouts stay
  INCOMPLETE; no step stop or replay mismatch inferred. The external shell lacked
  HIRSEL_IMAGE and the selected CLI had no adjacent hirsel-std.image, so source
  prechecking was selected by image.rs:406. The Buck harness has its exact matching
  image5efd4eed... (full hashes in deadline-image-setup.json). Missing-only host
  adapter cells use that evaluated tool environment; originals are not rerun.

## Final qualification dispositions

Application:1295 handwritten Hirsel lines added,0 removed: main285, spec224,
plan230, state207, run220, report129. Six real modules are explicit sources.
No std, compiler, runtime ABI or language change is included.

| Complete application workflow family | Reference | Checked H | LLVM | Cranelift |
|---|---:|---:|---:|---:|
| Main-entry declared fixed fixtures,57 identities |55 pass,2 Steps|55 pass,2 Steps|see native supplement|see native supplement|
| Native real-file/scripted-process supplement,54 identities/backend |separate world|separate world|51 pass,3 unrun|51 pass,3 unrun|
| Production real-child success/failure/cancel/deadline, widths1/2/4 |tape replay|tape replay|12 pass|12 pass|
| Final large-step numeric workflow, requested widths1/2/4 |direct+replay pass|direct+replay pass|3 pass|3 pass|
| Past-deadline immediate stop |2 direct pass+replay|2 direct pass+replay|1 pass|1 pass|
| Supplemental state crash cuts (Test entry) |1 pass|1 pass|1 pass|1 pass|

The four original main-entry tooling cells remain INCOMPLETE: capacity16 on
R/H at10M steps, and backward-clock on R/H at1M steps. No complete observation
exists for these cells and no bound was raised. The backward fixture's virtual
session is61 seconds; it ran no real minute wait and its original vector remains.
Cap/one-over config65536/65537, argv4096/4097 and state131072/131073 completed
under their initially declared finite bounds. max-runs33, jobs17 and deadline3001ms
are resource refusals, not language failures.

The native supplement uses real files and explicitly scripted status-only
processes, with independently sampled live clocks and shifted independent expected
occurrence minutes. It is not the identical fixed-clock simulated world. Injected
write/sync faults and Pending (three identities per backend) are unrun. The later
past-Pending native twins are likewise unrun. These eight cells and full-entry
native fixed-clock sequence/cross-width orchestration obligations remain INCOMPLETE.
The public current Fixture witness above executes the actual main and returns
usage2 because its Args/mount/file projections are absent. Owner:@agentlang-24,
message643764. No new runtime adapter or helper surrogate is provided.

All24 real-child production cells retained an application-ready socket handshake,
worker_phase=running, effective width equal requested1/2/4 and pending0, at logical
parallel4 and fixed HIRSEL_JOBS4. This proves ready execution capacity, not simultaneous
hardware execution. Direct children are joined; descendants are not promised stopped.
Large writes on both pipes are drained/discarded. Six short clock-free numeric
runs also compare complete Recording values across widths/backends, normalizing
only binding.admitted_at and binding.placement. They have no separate readiness
qualification and do not certify full timer/child workflows across fixed clocks.

There are134 complete application tapes and536 selected four-adapter replay cells:
530 passed,6 remain INCOMPLETE. The latter are four capacity16 host-adapter
no-observation results at unchanged10M and the two original width1 child-deadline
reference CLI15s setup timeouts. No step exhaustion or mismatch is inferred from
the timeouts. Matching-image missing-only deadline host cells passed10/10; all12
native deadline replay cells passed. Original grants/files/commands were unavailable
for replay; all tapes remained byte-identical and all events were consumed on pass.
No host effects were restored. Each live clock tape was compared with its own run.
Public host replay returns observations but no successful-run StepUsage; requested
bounds are retained, while direct reference/checked fixtures retain requested and
consumed step_usage outside observations/tapes.

All134 tapes were audited offline for stage sync -> rename -> published-file sync
before process spawn (18 original plus116 previously unaudited tapes). Twenty distinct
changed-input/config/Args/missing-event/extra-event controls refused on four adapters.
The original6 completed control identities were not repeated after the wrapper fix.
The Cranelift production wrapper's9 duplicate ordinary reference checks are disclosed
above and counted once. Setup reds, timeouts and Steps remain in the external ledger.

Independent pinned Go UTC oracle:12 next-occurrence expectations agreed. Separate
association-list state reducer:4 transitions agreed. Four-engine supplemental crash
cuts prove durable idle/admitted/uncertain/completed snapshots at the specified
persistence boundaries; they do not replace full main-entry workflows or claim
power-loss/transactional process launch semantics.

CLI source test inventory433 ->444,11 new cron_state_30 identities (6 normal,
5 explicitly ignored evidence helpers). Compiled harness inventory436 ->447;
these are inventories, not an all-harness execution claim. Named focused executions
and original failures are retained in the external logs/receipts. No existing app
fixture or unrelated module registration was changed.

Source and registration were frozen through all queued/live readers. Evaluated
BUILD_INPUTS, stamp reads/probes/globs/outputs, effective config and actual tool bytes
are sealed externally; partial/late initial seals remain labelled. Source boundary
comparisons find no changed owned source bytes. Formatting/late parser and deadline
fixes have their own generations; old immutable tapes bind their matching source
snapshot. No unchanged semantic greens were repeated because main moved or sealing
improved. FRICTION covers intended language rules, application bugs and tooling
separately. Reference deviations and MIT notice are in REFERENCES.md and LICENSES.

**Required obligations remain INCOMPLETE** for the exact tooling/provider/clock/replay
cells listed above. Useful application delivery is not all-engine/replay completion.
