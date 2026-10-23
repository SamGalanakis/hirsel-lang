# packtree authoring and verification

Base 8453bcf1e04d58bba305cfc3c74e6343669b041a. Original sources, seals,
oracles, failed drafts and runtime receipts live externally under
/workspace/code/hirsel-briefs/packtree-29. No runtime/compiler/std repair included.

1. Bounded discovery (intended API contract, ~15 minutes): file.read_dir returns
   the whole sorted directory without a bound. Using it for pack would allocate
   before member admission. The explicit bounded schema1 declaration is the
   application workaround, including every parent. Only target emptiness uses
   read_dir; caller promises an initially empty grant. Checking a hostile huge
   nonempty target can allocate a whole provider listing: that stronger resource
   guarantee is INCOMPLETE, owner packtree follow-up. No new general std API.
2. Owned task results (documented parhash recipe, ~10 minutes): nested
   Result[unique Member,unique Problem], with explicit unique copies at the task
   boundary in the first draft. Current read_one returns unique Member and
   moves it through the task result, avoiding that second payload copy. The
   generic exact stream still requests a deep unique copy of at most64KiB/file
   (<=1MiB/run); capture copies at most64 bounded entry paths. Problem crossing
   copies at most512 bytes/path plus the bounded error tag. Complete admitted
   payload remains retained until publication. Scope/Dir cross by existing
   rules; File stays branded to Dir.
3. Generic exact stream (ordinary application plumbing, ~10 minutes): exact[S,E]
   is instantiated for a branded File/FileError and a TAR body/ReadError[()].
   Both admit the byte count before growth, loop over short reads, probe EOF
   separately and distinguish short/long/source errors. No fake type wrappers.
4. Strict framing (application policy, ~10 minutes): std.tar intentionally
   accepts a missing terminator. Block-sized source refills expose the two zero
   blocks without a second header codec. Extra zero record padding is allowed;
   nonzero trailing bytes are refused. Header/PAX parsing remains entirely std.
5. Cancellation readiness (known provider gap, inspection ~5 minutes): minimal
   required app is two declared regular files/jobs2; first worker waits in
   file.read, second errors, then cancel_all joins the first and queued work
   never starts. Public SimulationConfig faults inject Canceled but cannot
   script a pending regular-file read. FIFO substitutes violate the app policy.
   This exact acceptance cell and native cancellation/cleanup at widths1/2/4
   remain INCOMPLETE; owner packtree acceptance / shared E1 owner via agentlang-24.
   Injected canceled declaration reads are mapping tests, not worker cleanup.
6. Checked-H host-tape replay has no public adapter; UNSUPPORTED/INCOMPLETE.
   Checked H must execute the complete small scripted matrix separately.
7. Attempt01 rejected reserved `before` (LANG-14, manifest.hir/unpack.hir).
   Expected intended precondition keyword; renamed to previous_count, no runtime
   cost (~1 minute). Harness originally used Debug for load errors and printed
   the entire source map; Display now retains the focused diagnostic only.
   Attempt01 selected1, started1, failed1, workflows0. Original log external.
8. Attempt02 check (intended ownership/names, ~5 minutes): generic callback
   can share its mut byte buffer, so exact's return requires an explicit deep
   unique copy (<=64KiB per body); concrete admit/make_member now state unique
   results and keep owned inputs. Seen entries copy only bounded path metadata.
   LANG-4 also refused member/problem locals shadowing module functions and a
   data local shadowing scan's input. Renamed make_member/failure/payload.
   These are application contracts, not language defects.
9. Attempt03 (intended, ~3 minutes): failure names a core module, so issue is
   the error encoder parameter. `unique entry` consumed its owned binding;
   `unique copy entry` preserves the member for its body. The local append
   helper now exports @preserves_unique(out), matching its only bytes.append
   operation, so encode needs no whole manifest copy.
10. Cap partition first attempt: members64 reference reaches the unchanged
    1M-step ToolingLimit at std.tar.hir:221:5 before completion. Original full
    app vector and failure retained in attempt07/test.log. Cap tests retain
    ToolingLimit as INCOMPLETE, separately from domain-error passes; no smaller
    case or raised step budget receives its credit. Later runs partition only
    missing cells. Exact per-engine dispositions are in the external receipts.
11. One Linux name component is limited to255 bytes; the512-byte TAR path cell
    therefore lists rather than creates that single-component name on the real
    provider. Path admission still uses512 UTF-8 bytes; the stronger portability
    of arbitrary component sizes is not promised. Multi-component extraction
    tests retain real byte equality. No confinement fallback.
12. Test08's full payload1048576 simulator world reached the test launcher's
    HIRSEL_MEMORY_LIMIT=7808M, allocating58,720,256 more bytes at a resident
    peak 5781M; the process ended before an observation. This is resource refusal,
    not a language failure or semantic pass. The input remains1MiB; no cap is
    raised. Reference full payload is FAILED/INCOMPLETE; checked-H payload and
    payload-one-over plus archive-exact simulation remain NOT_RUN/INCOMPLETE
    because the same whole-world snapshot shape exceeds the bounded process
    envelope. Native retains their exact vectors. Future test processes use
    explicit2GiB. Owner packtree acceptance/tooling; no incidental engine repair.
13. Buck2 does not inherit arbitrary shell variables into test actions. The
    initial PACKTREE_29_SKIP/EVIDENCE_DIR settings were absent, so test08 repeated
    members64/reference and did not retain scratch at the intended evidence path.
    The original test07 limit and all test08 receipts remain separately archived;
    subsequent partitions pass these settings via the documented executor --env.
    No repeated cell is counted twice. About5 minutes; harness setup, fixed.
14. Directory-size oracle initially expected path dir, but Python tarfile stores
    directory names with a trailing slash. The app's diagnostic preserves the
    exact input dir/ while successful manifest keys are canonical. Corrected
    that oracle only (~1 minute); test09 prior cap greens remain credited.
15. Go's nine-member filepath tree additionally reaches the original1M Steps
    limit in std.tar.hir:220:9 (test10 reference). It remains an independent
    unchanged vector, not a replacement for members64. Checked-H is attempted
    separately. All 8 prior test10 observations remain green. Expected tooling
    limits have explicit identities; an unexpected limit still fails the harness.
16. Native01 completed55 LLVM production launches before duplicate depth9 IDs
    (workflow and cap recipe) collided in the evidence directory. No duplicate
    app ran. Driver now deduplicates identical IDs and supports resuming from
    completed immutable receipts/binaries/tapes; it verifies original app/tool
    hashes, seals the changed driver separately and never repeats a completed
    launch. Original driver and setup failure remain external. About5 minutes.
    Metrics files for short launches can be empty; requested widths alone are
    never claimed as effective-width proof.
17. Original native02 continuation failed before app execution because its old
    compiler path was relative to crates/hirsel-cli. Native03 continuation copied
    binaries without executable permissions and also ran no app. Separate fresh
    native04/05/06 directories preserve all original55 passes and raw attempts;
    executable mode and original cwd are now explicit. No pass is counted twice.
18. Reference host-tape replay refuses members64/65 at UnconsumedInput(17),
    file65536 at(32), payload1048576/1048577 at(267), walk-tree at(58), both tape
    origins. These are exact known ID/error allowlists, never arbitrary status3
    acceptance. Two-MiB zero-archive LLVM-origin replay timed out at unchanged60s
    (629MiB/2GiB); Cranelift-origin counterpart is NOT_RUN after that finding.
    All 14 required reference replay cells stay INCOMPLETE. Finish-before-project
    is a hypothesis, not proved Steps attribution. Owner interp-step-budget-29;
    external replay-witness/ and ALLOCATION.md retain original source/tape/tools.
19. Stable-root native qualification adds 12 success/domain recordings, requested
    widths1/2/4, logical jobs4. Control creation/inotify and socket replies are
    handshakes;10 cells observed effective=requested, LLVM success1 stayed0 and
    Cranelift domain2 missed its sample. All 12 native outcomes and24 native
    replays pass. All 12 reference replays refuse with exact known cursors22
    (success) or10 (domain); these are NEW replay findings with unproved cause,
    not original budget-stop passes. Expected/seen bytes remain external.
20. Full typed recording and canonical trace audit passes12/12 stable-root cells
    across both backends and widths. Only admission timestamp and placement are
    normalized; no task IDs/errors/clocks/events/data are dropped or sorted.
    Audit01 is a read-only test, never an engine run or additional replay.
21. Sealing: attempt 01's evaluated sync stamp/tools were supplemented after the
    consumer (late seal), and the exact late tools are labelled attempt 06/
    late-tools.json. Later consumers retain evaluated stamp plus owned/tracked
    source/generated-output/local-config hashes; native prebuild tool hashes and
    per-generation binaries/grants/tapes are retained. Documentation-only edits
    follow completed consumers. No evidence is committed as application source.

22. Copied binding tamper controls use the shipped recording codec and validated
    canonical-run reader. All 9 argument/config/truncated-tape controls pass on
    reference/LLVM/Cranelift with asserted refusal reasons, empty stdout, absent
    original roots and byte-identical originals. The earlier changed-config
    CLI-shape refusal is retained as syntax-only evidence, not binding credit.
23. Placement supplements use separate64KiB inputs, not rewritten original
    cells. Qualified LLVM success1 observed effective1. The first Cranelift
    domain2 supplement's109-byte Unix socket name exceeded the client's Unix
    path limit; shortening its private control pathname to ctl.sock observed2.
    Both original missed snapshots stay missed; supplemental actual outcomes 3
    and native replays 6 pass. Their three reference replays have NEW exact raw
    UnconsumedInput refusals with unproved cause and remain INCOMPLETE.
24. Launch/test receipts: CLI harness identities384 ->387 (three new tests).
    Small matrix 86 app observations PASS; distinct additional cap observations 22
    PASS (24 executions include depth9 already covered by the small matrix).
    Eight exact ref/H step-limit cells plus six resource/unrun cells INCOMPLETE.
    Native main matrix120 production outcomes PASS,240 native replays PASS,
    106 reference replays PASS,14 reference replays INCOMPLETE. Qualification adds
    12 outcomes/24 native replays PASS,12 reference refusals INCOMPLETE; supplements
    add3 outcomes/6 native replays PASS,3 reference refusals INCOMPLETE. Typed12
    audit and9 copied binding controls PASS. These are different evidence kinds,
    not one synthetic all-green test total. Canceled waiting regular-file worker,
    native cancellation qualification, native scripted file-fault recordings and
    checked-H host-tape replay remain unsupported/INCOMPLETE. The simulator seam
    and allocation/refusal findings have owners; no duplicate repair is included.
25. Documentation shell setup (~1 minute): a nested heredoc prematurely ended
    the edit script, so it rejected before changing files and the illustrative
    missing CLI paths returned127. No app ran; distinct heredoc delimiters fix
    the authoring command. This is not language or application evidence.

## Bounded directory iterator update

The target-emptiness listing allocation limitation in item1 is resolved by the read_dir iterator cutover: the provider retains a fixed128-entry page and extract pulls one fallible entry before rejecting a nonempty target. Manifest-driven pack discovery and the other recorded obligations retain their existing scope.
