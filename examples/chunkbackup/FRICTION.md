# chunkbackup qualification

Base ff3886a3b2e1dec2912efca90e4d995fa38f070a. External matrix, exact drafts,
seals and immutable receipts: /workspace/code/hirsel-briefs/chunk-backup-31/.
No runtime/compiler repair or language-rule change is included.

Application design: bounded explicit manifests avoid scanning a filesystem.
Each of four file workers transfers one owned chunk into a child insertion task
and awaits its durable result before continuing; these lexical credits bound
queued/in-flight chunks at four. Cancellation joins every insertion and reader.
This has no unbounded producer channel; ordinary application plumbing.

Reference: restic/chunker 2e8f53f1dad5dbdcf8aca1dbf1bee7d1fc9d8ce8,
chunker.go:106-249 and chunker_test.go:64-70,189-240; BSD-2-Clause retained in
LICENSE.chunker. Fixed custom API configuration 256/1024/4096, 64-byte window,
polynomial 0x3DA3358B4DC173. Deviations: no global mutable table cache; eight
linear basis polynomials build the identical outgoing table. Borrowed file
windows and an owned <=4096-byte chunk replace Go's Reader/append buffer.

1. Draft01 rejected missing prefix result annotation in restore (LANG-2),
   implicit nonunique generic Part result (MODE-2/CLO-3), and parent Scope
   captured into a worker (MODE-6). Expected intended rules. Explicit Result
   annotation, unique descriptor producer/consumer contract, and each file's
   own nested scope resolve them. No byte copy required at the transfer seam.
   ~8 active minutes, application owner, fixed; draft01/ and diagnostics01.log
   preserve exact attempted source and rejection. Initial harness1 failed1,
   workflow0; Display from check_for_execution omitted detailed diagnostics,
   so use CLI check for the full source report. No adapter repair.

2. Draft03 destination inventory pushed path.text's aliased String into a
   list popped under a unique contract (MODE-2). Explicit unique copy of each
   <=1024-byte directory path fixes it, <=128 paths/run. Intended rule, ~2
   minutes, application owner/fixed; draft03 and diagnostics03.log. The initial
   test-executor --env placement was a setup refusal (0 tests), corrected by
   placing it after --. Script02 selected1/failed1/workflows0 at source check.

3. First full app script03: 18 reference/checked cells pass; AfterWrite release
   reference remains Blocked after the sibling's successful Durable put_new.
   Current provider intentionally triggers only on file.write_bytes (stdlib
   runtime README contract); an immutable app cannot release it with put_new.
   Preserve exact fixture/source/tools and original failure under scripted-hNeHHp
   and script03.log externally. No mutable marker or injected cancellation is
   substituted. This required release cell is INCOMPLETE, provider scope routed
   to agentlang-24; ~5 minutes. The checked-H and both native counterparts also remain Blocked. Pending read cancellation did actually park, join and close.

4. Native01 compiled both backends and linked real Args fixtures, but its
   first tiny LLVM launch never entered: tools/capped's /usr/bin/env python3
   under launch_fixture's env_clear yields sys.executable=""; systemd reports
   "Failed to find executable : Is a directory". Setup failure, zero native
   semantic credit. Explicit /usr/bin/python3, PATH and 2GiB/4-CPU wrapper env
   repair the test recipe, no runtime change. ~3 minutes, application tooling
   owner/fixed; native-K4DdmL and native01.log retain source/fixture/tools.

5. Script05 completed30 missing reference/H cells then its verify-corrupt
   assertion incorrectly required a deliberately corrupt initial fixture chunk
   to hash to its name. Restrict that assertion to newly-created orphan chunks;
   verify correctly refused the corrupt input and preserved it. No app change.
   ~2 minutes, harness owner/fixed; script05.log retains the original failure.
   Native02: tiny and actually Pending sibling-cancel pass on both backends,
   each tape replays in both native engines and reference/checked H offline.
   AfterWrite remains Blocked on both native backends (original receipts retained).
   Native02 linked-artifact digests were missing before launch; later supplemental
   hashes are labeled late, and missing native identities add pre-launch hashes.

6. Production01 success/domain: all12 requested LLVM/Cranelift width1/2/4
   runs pass, with settled running/effectiveN/pending0 receipts and24 native
   replays offline. Its six intended cancellations completed successfully before
   a signal was delivered, so remain INCOMPLETE; cancel02 has the same6 missing
   handshakes. Canonical trace files publish only at completion, and cannot
   handshake a live read. cancel03 fixes only the observer: a completed flushed
   v8 Signals projection frame (sequence/CRC32C checked) and SO_PEERCRED PID.
   All6 cancellations now exit2/canceled with no snapshot, settled widths and12
   native replays; prior successes and original incomplete receipts are retained.
   ~10 minutes, application qualification/fixed; external production*/ receipts.
   Capacity is not proof of simultaneous CPU use. Jobs stays4 and Free is off.

7. Bounds01 grouped several large fixtures in one harness process. Before
   reaching60 CPU-s its process received an explicit60:61 RLIMIT_CPU; it stopped
   during manifest-cap checked H, with no observation for that cell. Preserve
   the original CPU stop; no larger cap or smaller input replaces it. File-cap
   and total-cap both hit exact40M Steps in reference/H. Nine other boundary
   observations completed (including manifest262144 reference at38,347,347).
   Remaining unstarted identities are partitioned, fixed budgets unchanged.
   Subsequent evidence processes opt into a fixed60:61 CPU cap before execution.
   Harness partitioning/tooling, ~3 minutes, application owner; bounds01.log and
   scripted-qA2YM7/. The original grouped CPU-stop cell remains INCOMPLETE.

8. Native03 completed all6 duplicate width/backend recordings with full-order
   semantic equality (only admission/placement normalized),36 native replays and
   12 host replays. input-hash/LLVM/w1 completed live,6 native replays and its
   reference replay, then checked H hit the process60CPU stop. The existing
   immutable tape is used to recover only that missing checked replay; no live
   input-hash/w1 or duplicate repeats. Remaining widths/backend identities are
   genuinely unstarted. Preserve native03.log/native-3x5OQh; subsequent adapters
   persist each receipt before the next starts. Application tooling, ~4 minutes.

9. native-group0 still grouped five workflows and hit60CPU during zero256
   LLVM checked replay after7 live observations and their preceding adapters.
   Exact receipts/native-bHzMnA remain. All subsequent live recovery is one
   workflow (and large boundaries one recorder backend) per process, with
   fixed60CPU/2GiB; missing checked halves use their original tapes. No accepted
   live workflow is repeated. ~3 minutes, harness partitioning/fixed.
10. Matrix-initial.md said "output/grant failure exit3" before implementation.
    This was planning shorthand, corrected explicitly before any OutputFault
    vector ran: main returns Result[ExitCode,Output(OutputError)] and propagates
    out.write/err.write errors, so PROG-3 defines exit1. Missing stdout/stderr
    grant is an explicit Ok(exit_code(3)). No failed exit3 oracle/red was changed,
    and no new runtime qualification is claimed for this contract correction.
    Intended entry contract, ~2 minutes, application owner/documented.

11. Controls01's changed-input and binding/header refusals passed; extra-event
    reference/H refusal passed, then the native oracle wrongly required empty
    stdout after final event-consumption refusal. Native had already replayed
    its exact successful output prefix, then refused the extra event with exit3.
    Correct only that named expected prefix. Preserve control files and red;
    rerun only missing native extra-event, truncation/checksum and a separately
    named valid-canonical config mutation. The initial config mutation used
    reordered JSON, so no canonical configuration-binding credit is inferred.
    ~4 minutes, harness owner/fixed; controls01.log and external controls/.

12. Default harness invocations now dispatch each exact case/engine or native
    recorder into its own60CPU/2GiB child, verify one named test executed, and
    propagate child failure. This makes the shipped command bounded without
    depending on a developer's manual partition. A new unrun deny-output case
    qualifies this dispatcher without repeating an accepted live workflow.
    Child fixtures use the same public full-entry adapters and fixed budgets.


Final qualification accounting (immutable per-cell ledger outside the repo):

| Family | Completed semantic passes | Incomplete disposition |
| --- | --- | --- |
| Full app reference | 61 plus61 exact simulated replays | seven Steps; one AfterWrite Blocked |
| Full app checked H | 61 plus61 exact simulated replays | seven Steps; one AfterWrite Blocked |
| Real splitter fragmentation/reset | reference and H,2 total | none |
| Scripted native recordings | LLVM72, Cranelift72 | AfterWrite Blocked on both |
| Scripted native tape native replay | 316 adapter receipts | four Blocked-tape refusals, separately retained |
| Scripted native tape reference/H replay | 260 complete (includes two recovered H halves) | 28 exact Steps; four Blocked-tape adapter errors |
| Production recordings | 18 success/domain/delivered-cancel qualifications, plus2 real publication faults | 12 older undelivered cancellation attempts retained |
| Production tape native replay | 64 complete, including old undelivered attempts | none |
| Production tape reference/H replay | 8 complete | 52 exact40M Steps; four adapter errors |

The14 direct Steps cells are seed8192/shift8192/shift-pair at20M, and
file-cap/total-cap/chunks2048/chunks2049 at40M, each reference and H. Each
native recorder completes these fixtures; its corresponding28 host tape replay
cells stop at the same original bounds. No stopped identity is a semantic pass,
and no bound was raised. Two provider-fault workflows each have two UNRUN native
engine cells: public native Fixture has no file-fault queue. This is a demonstrated
adapter surface gap, not a claim that either engine cannot implement file errors.
Real host chunks0555 under an RW grant gives exit2/chunk_publish with no snapshot
on both backends; it supplements those four unrun scripted fault cells.

The AfterWrite full app witness is source/a waiting for the sibling's actual
chunkSHA(def) Durable put_new. The public contract (hirsel-interp library README,
AfterWrite readiness) triggers only file.write_bytes. No mutable marker was added.
The two retained Blocked tapes each fail both host replay adapters with literal
"interpretation is not implemented yet"; all four standalone native replay
attempts refuse Io MissingInput(35), exit3. These are adapter errors, not Steps,
and replaying Blocked would still not qualify the required successful release.
Exact source, Args/grants/tapes/tool hashes are in external scripted-hNeHHp,
scripted-HH3Nds, native-AOeQHc and blocked-replay; routed to agentlang-24.

Production delivered-cancel LLVM width1 and width2 tapes each return
"interpretation is not implemented yet" in both host adapters (four cells),
while LLVM width4 and Cranelift widths1/2/4 replay in both (eight passes).
All12 native replay adapters of these six delivered-cancel tapes pass. The
52 production Steps are36 original tapes,12 cancel02, and4 publication-fault
host cells. These four adapter errors are distinct from Steps and from the
four blocked-tape host errors; all are retained and routed to agentlang-24.

The typed Pending audit checks a real FileRead Blocked event, Close of that exact
handle, no Open of queued c, no PutNew, and zero native leak audit. The18 retained
success/domain/Pending tapes across two backends and widths1/2/4 compare in original
order, with only binding admission time and placement normalized. Production uses
ordinary Ordered, fixed jobs4 and a constant4-CPU wrapper; running/effectiveN/
pending0 receipts precede completion. No simultaneous hardware claim is made.

Refusal controls have24 exact engine receipts: changed-input, initial reordered
configuration/header, valid canonical seed mutation, extra-event, truncation and
checksum, across all four adapters. Valid seed mutation refuses run_identity;
extra-event may emit the recorded output prefix before refusing unused events.
No live recording was repeated for control recovery. Original controls01 red and
its first ten successes are preserved.

Early tool seals omitted some linked-artifact/LLVM-library/kit/floor hashes. Late
supplements are labeled late-native02-and-production-receipt-seal; later consumers
have preconsumer evaluated stamp reads/probes/globs/outputs, source/config/tool
and linked-artifact hashes. These late seals do not retroactively qualify original
boundaries. Original setup failures, groupedCPU60 failures and cancellation
handshake failures remain in raw logs, with only unfinished halves recovered.
The dispatcher packaging cost is one fresh harness/tool setup per bounded child;
no performance or compiler/runtime/std repair claim is made.

A publication error can follow an established filesystem effect: completed counts
only confirmed whole-file successes, so an uncounted failing file may nevertheless
exist completely. This is the file provider contract, not multi-file rollback.
Five new test identities extend the base CLI harness492 to497. All five identities
executed nonzero focused selections; exact per-cell results above take precedence
over aggregate harness exits. Source is1198 lines across main95/manifest232/
chunk131/store113/snapshot349/restore278, with no unrelated app/std changes.
