# journalkv friction and qualification

Implementation started at 8453bcf1e04d58bba305cfc3c74e6343669b041a.
Evidence, immutable drafts, hashes, matrix, receipts and raw recordings live
externally in `/workspace/code/hirsel-briefs/journal-kv-29/`.

1. **Missing nontruncating append-open (intended API boundary; ~15 active minutes).**
   Minimal original `original-open-witness.hir` opens a durable existing log with
   `file.open`, then `file.append`. Core contract769 and interpreter mode checks
   return FileFault; `file.create` would truncate. Expected application seam is
   reopening without losing history. Owner decision639842 authorized the separate
   `file.open_append` prerequisite, not an ownership relaxation. Its additive
   request tag 114/native ABI 50 preserve old modes. No extra application copy.
   Owner: journal-kv-29, coordinating mc-free-f1-17; disposition fixed by landed ac95beba38231c7193bbc1780fb7b6583f7fc272.
2. **Native declaration review omitted in first prerequisite draft (harness setup;
   ~5minutes).** Exact diagnostic: `hirsel: native manifest: missing native
   declaration identity: file.open_append` (append-test-01). Reference/checked H
   completed4semantic cells; native built no app. Add the new independently
   reviewed signature/ownership row; existing rows unchanged. No runtime cost.
   A later native-inventory generation attempt found an omitted exhaustive
   `Request::OpenAppend` arm in pipeline test rt_replay.rs:151 (E0004). Add its
   translation beside Open/Create; preserve setup failure and recover missing
   tests only. Owner: this lane; intended additive-enum maintenance.
3. **Task results and borrowed decoding (ordinary plumbing, no defect claim).**
   Result types with Strings use explicit unique results at child boundaries;
   prepared bytes own their storage, while CRC/header parsing borrows spans.
   `unique` fallback copies are bounded by declared snapshot/log caps. Shared
   snapshot loans remain inside `shared.with`, and all handles join before error
   propagation. Caller owns every append/commit/reopen/sync and response.

Application receipts and remaining incomplete obligations are listed below;
prerequisite runs are separate from full-app qualification.

4. **Reserved names and module heads (intended; ~5minutes).** Draft check02
   rejected local `result` (LANG-14). Draft03 reported `table.entries` as access
   through the std table module and rejected `entry`/`payload` locals shadowing
   module-level functions (LANG-4). Use `recovered`/`latest_state`, and rename the
   constructors/helpers to `make_entry`/`encode`. No allocation or behavior cost.
   Exact failed drafts and diagnostics: draft-check-02/03-source and
   logs/journal-kv-29-draft-check-02/03.log. Owner: application; intended.
5. **Nested uniqueness and whole-owner movement (intended; ~10minutes).** Draft03
   `task.await` returned an outer unique Result whose payload type was merely
   Bytes/Recovered. MODE-2 correctly rejected receiving it as a *nested* unique
   payload. Use `unique Result[unique Bytes,E]` and explicit nested qualifiers on
   the recovery/command producer results. An attempted `take recovered.state`
   was OWN-8; a qualified record pattern was syntax-invalid, and an unqualified
   foreign-module record pattern did not resolve. The defining module exposes
   `recover.into_state(take recovered)` and destructures its own whole record.
   No ownership relaxation; generic index payloads move rather than copy.
6. **Prefix inference and type-argument syntax (intended; ~5minutes).** Draft05
   `let change:Mutation=match codec.decode(payload)` could not infer the generic
   call from inside match (LANG-2). Draft06's `codec.decode[Mutation]` is forbidden
   expression syntax. Bind `let decoded:Result[Mutation,DecodeError]=codec.decode`
   then consume-match it. Match constructors in CLI output similarly need an
   explicit `json.Value` result annotation. No runtime copy or codec changes.
7. **Moved compiler's dynamic library (tooling; ~1minute).** Copying a CLI artifact
   out of Buck's tree broke its relative LLVM RPATH (libLLVM.so.23.1 absent,
   exit 127). Explicit LD_LIBRARY_PATH to the pinned LLVM lib tree repaired only
   the external draft check. Original failure preserved; no semantic run credit.

Draft07 passed source check with the unchanged toolchain at ABI 49 plus its new
operation declaration. This is a static diagnostic pass only, with no app
workflow or native/replay credit. Final runtime consumers use the landed matching
ABI 50 toolchain and their own source/tool seals.

8. **Optional interrupt-race result annotations (intended; ~8 minutes).**
   Draft08's nested Task/TaskSet result inferred merely counted Bytes and was
   rejected with MODE-2 at task.select. Draft09 explicitly annotates both worker
   and watcher as `Task[unique Result[unique Bytes, record.Error]]` and the same
   TaskSet payload. The worker checks cancellation per snapshot row; the signal
   watcher returns the same typed result and select cancels/joins the loser.
   No extra copy, special scheduler, or ownership relaxation. Owner: application.
9. **Test-only ownership diagnostics (intended; ~5 minutes).** The first crash
   helper used reserved local `before` (LANG-14). The next moved `first`/`original`
   into test.dir and reused them (OWN-2); explicit copy shares initial immutable
   fixture bytes. Its assertion retained a share of recovered state across a
   unique handoff (MODE-2); the helper uses `unique recovered`, a bounded deep
   copy for that assertion. Production hands the recovered owner directly to
   compaction. Failed sources integration-check-01/03-source.hir and logs are
   external. Owner: test application; no language defect claim.
10. **Observed evaluator guards and buffer growth (tooling/owner fix; ~10 minutes).**
    Boundary values4096/4097, commands64, live64/65, records256/257 and log1MiB
    hit the unchanged 1,000,000-step evaluator guard in reference and checked H.
    The full 1MiB command file then exhausted the old observed adapter's memory
    reservation: resident peak6890M, allocating58720256, EXIT32. The process
    stopped before its checked-H and one-over cells; no semantic pass is claimed.
    Minimal actual-app witness is caps.json batch-1048576: `[]` padded with spaces
    to1048576 bytes. Owner interp-step-budget-29 identified accumulated-buffer
    request events and COW clones in Library::file_stream, and landed
    35354f437bf4712b317bee683465df8ee3e895f6 with 2/4/8-read storage-reuse,
    shared-prefix and exact replay regressions. No parallel repair or unchanged
    1MiB rerun; original disposition remains INCOMPLETE. The default large
    boundary Rust test is explicitly ignored and must be selected in an isolated
    capped process at 2 GiB; it never counts guards as semantic passes. Ordinary
    CLI reference replay and both native backends completed the same full bytes
    under2GiB; those replay passes do not replace checked-H observed execution.
11. **Native scripted fault provider absent (unsupported; ~5 minutes).** The
    production standalone native grants interface has no public file-fault queue,
    and `--simulation` excludes `--native`. Nine scripted partial append, sync,
    atomic write/commit/open and canceled file calls execute the actual main in
    reference/checked H only. Native/full-engine cells remain INCOMPLETE. Public
    Test helpers cover native crash/stale handles and explicit cancellation, but
    are supplemental, not replacements for these exact main-entry workflows.
    Owner: existing runtime adapters; no incidental adapter added.
12. **Physical admission snapshot timing (harness; ~5 minutes).** Four first
    production width4 snapshots observed effective2/3, pending4. Their semantics
    and replays passed but placement was unproved. Recover only those four cells
    with repeated socket metrics responses until effective4, before signal
    delivery; no sleeps/test-only scheduling. All four settled and passed.
13. **Cross-width semantic identity (harness; ~5 minutes).** First recordings
    used a distinct granted host directory per width. Full Recording equality
    correctly differed in canonical grants/run identity after normalizing only
    admitted_at and placement. Preserve 0/8 comparisons as setup, not a runtime
    defect. Replacement width2/4 cells reuse the width1 host paths with identical
    initial bytes. No IDs/errors/clocks/events are sorted, stripped or normalized.
14. **Late supplemental artifact hashes (evidence limitation).** The initial
    source seal precedes the main matrix; exact executable/std image hashes were
    supplemented at the next boundary. Do not claim these late hashes were early.
    Between matrices only harness/support, index attribution comments and docs
    changed; production algorithm bytes stayed fixed. Complete later source/tool
    seals and post-boundary comparisons are external. No repeated green workflow
    solely for seal timing or main movement.
15. **Additional control and helper setup (harness; ~8 minutes).** The initial
    changed-config audit serialized a sorted JSON Value, proving the intended
    noncanonical-header rejection rather than valid changed binding. Preserve it
    under that label; the corrected test clones the typed public bound run,
    changes stdout, calls validated()/canonical(), then checks identity refusal.
    A direct Rust test launch first used the repository root instead of the
    harness crate cwd (stdlib manifest NotFound, zero execution). The first
    external native Test invocation lacked LLVM_SYS_231_PREFIX (native link
    refusal, zero tests); the focused Buck-native helper inherits the evaluated
    toolchain environment. An audit draft named hirsel_run_config without a
    direct test dependency (E0433, zero tests); use the existing public
    Recording.bound_run().record().clone().validated() surface, adding no API.
    The initial different-source Cranelift-labeled control used the LLVM append
    binary again; retain that as one duplicate and run the missing actual
    Cranelift executable control separately. Owner: this lane; no language fix.

## Qualification receipts

Handwritten application:809 lines (main81, record214, recover122, index100,
compact103, session189). Six modules are explicit manifest sources. Tests,
fixtures, licenses and documentation are excluded from that count. The
independent association-list/SER-2/bitwise-CRC oracle and initial expected byte
fixtures were frozen externally before application execution.

| Actual complete application cell | Reference | Checked H | LLVM | Cranelift |
|---|---:|---:|---:|---:|
| Small workflows, including corruption/torn/batch errors |48 pass|48 pass|39 pass|39 pass|
| Scripted file faults (included in48 above) |9 pass|9 pass|unsupported|unsupported|
| Cap/one-over workflows |5 pass,8 step limits,1 memory stop|5 pass,8 step limits,1 unrun|14 pass|14 pass|
| Production Ordered success/error/cancel, widths1/2/4 |not this adapter|not this adapter|9 pass|9 pass|
| Supplemental public Test crash/cancel helpers |4 pass|4 pass|4 pass|4 pass|

The small reference/checked-H harness loads the actual manifest/modules and main,
projects Io grants through SimulationConfig and compares complete observations,
returned error variant, files, stdout, task/effect order and sync-before-ack.
The native matrix checks full output/exit/error and independently expected bytes.
Helpers exercise the same recovery/compaction modules with public Test/crash,
not substitutes for main-entry workflows.

There are124 distinct native main-entry workflow cells and372 corresponding
reference/LLVM/Cranelift tape replays, all passing. Original mutable directories,
command files and grants were removed before every replay. Tapes remain byte
identical; an unconsumed/missing event refuses. Four admission-proof recovery
cells and eight equality-setup recovery cells add12 native executions and36
replays; retain them separately, not as new functional coverage. The latter
helper unnecessarily repeated semantic/replay checks while repairing only the
comparison setup; this is disclosed rather than counted as extra obligations.
All18 placement obligations now have effective1/2/4 control replies at jobs4.
No claim is made that a pure snapshot physically used every admitted worker.

Real cancellation waits for the complete io.signals recording event, then sends
one Interrupt through a pidfd for this lane's own process. No sleep or test-only
scheduler is involved. All six cancel cells return typed Canceled, no output,
unchanged old journal, no temporary; every child is joined. Each live tape is
compared against its own replay, not a separately sampled signal clock.
The eight deterministic success/domain cross-width comparisons match complete
public Recording values after normalizing only binding.admitted_at=0 and
binding.placement=None. Original distinct-root comparisons remain0/8 setup
failures, not normalized away.

Refusal controls cover changed input, noncanonical config, frame checksum,
missing event and changed program; original failures and control tapes remain
external. The first changed-source native audit accidentally used the same LLVM
prerequisite binary under both adapter labels:14 distinct controls plus one
duplicate, not15 distinct controls. A separate canonical-config binding control on all three adapters
and the missing different-source Cranelift executable control complete the
correction:18 distinct refusal controls plus one duplicate run. Canonicalization uses public RunConfig validation/canonical bytes;
no new codec or replay adapter is introduced.

**Required obligations remain INCOMPLETE.** Sixteen cap cells hit the unchanged
1M evaluator guard, the observed 1MiB batch stopped on memory, and its checked-H
cell was not run. No smaller input or CLI replay is called their pass. Nine
scripted fault workflows per native backend have no public production fault
queue adapter. A checked-H host-tape replay adapter is unavailable. Free remains
OFF/unqualified. The buffer-growth owner fix is landed, but this lane does not
rerun unchanged 1 MiB inputs or claim post-fix semantics it did not execute.

External witnesses and receipts are rooted at
`/workspace/code/hirsel-briefs/journal-kv-29/`: `MATRIX.md`, `oracle.py`,
`app-reference-01`, `app-native-01`, `caps-native-01`, `boundaries-01/02`,
`integration-01/02`, `ordered-01`, `ordered-recover-02`, `ordered-equality-03`,
`ordered-merged`, and combined-native-counts.json. Exact source/config/fixture
and tool hashes accompany the boundaries; raw stdout/stderr and immutable tapes
are retained there. Setup rejects (missing toolchain env, wrong harness cwd,
private test entry, guessed missing shard) have zero semantic execution credit.

There were zero journalkv Rust identities and no application at the base. The
prerequisite added3 named Rust tests; the application adds7, for10 journal
identities (three explicitly ignored evidence/cap campaigns). Source inventory
in the CLI directory is381 at the original base,385 at the prerequisite main
snapshot (including one unrelated incoming test),392 now. The built CLI harness
reports395 identities including tests declared outside that directory; these
are inventory counts, not a claim that the whole harness ran. Only the focused
identities above ran. The boundary matrix's original named test failed and the
remaining one-over partition passed; evaluator guards remain incomplete.
