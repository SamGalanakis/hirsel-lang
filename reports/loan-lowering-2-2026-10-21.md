# Loop, pattern payload and joined-view repair

All three historical families now pass: the original wc2 file loop, all33affected tar programs with unchanged assertions, and nested joined global views. The wc2 application again reads files inside its outer source loop; its reader-helper workaround is removed. The original repro files remain unchanged.

The baseline was origin/main0a0d9aed835ce1808f18342a139835935187df05. Work used an isolated checkout. Raw evidence, source certificates and manifests are retained under `/workspace/code/hirsel-briefs/loan-lowering-2/evidence` and `/tmp/loan-lowering-2`. Landing identities and the final diff counts are recorded in the external disposition after `tools/land` completes.

## Mechanisms and prior art

Rust reference a7c37f9f0ee5e93396106e9de20f4a86b4154e63:

- `compiler/rustc_mir_build/src/builder/expr/into.rs:23,105–128`: nested expressions lower into a receiving destination and join at a block.
- `compiler/rustc_mir_build/src/builder/scope.rs:1383,1470`: value destruction belongs to its owning scope and is separate from storage end.
- `compiler/rustc_mir_build/src/builder/expr/as_place.rs:308`: projections carry physical field types.

Hirsel's deviations follow its value-mode and brand semantics. Global content can still require access loans, so block extraction promotes the permissions held by its returned view. Physical pattern payloads preserve J-substituted identities but use declaration locality; receiving bindings retain their separately checked receipt types. The additive `OwnershipFacts.physical_patterns` field is covered by pre-release format standing approval. No TIR/LIR construct or native ABI changed.

Source replay recreates owners declared inside loops. Renewal removes a previous incarnation's expiry only for rows local to the current lexical item/body, not rows held outside or referring to external origins. Escape and move twins remain rejected.

Native verification additionally found that whole borrowed assignments of existing joined SSA views lost their classification. After coordination with the borrowed-encoding lane (messages583776,583826,583833), the importer follows a forwarding assignment only to a proven alias or joined view. It preserves the existing holding, invents no loan and takes no SSA address. The ordinary borrowed-storage rejection guard, C and H are unchanged. A forged graph stripped of original loan evidence still rejects at that guard.

Go reference ebe2feefc400f813aefdc8a036e66d2619da6649, `src/archive/tar/reader_test.go:793` (`TestReadTruncation`), supplied the original tar vectors. Existing truncation, short-refill and GNU-incremental assertions were not edited.

## Authorized validator precision correction

The original brief required unchanged V. Consultation583154 received explicit orchestrator authorization in message584632 to correct natural-loop membership, retain all rejection guards, preserve conservative irreducible handling, add rejected controls, and use a separate commit. That authorization is recorded in commit8f28570d's body.

Before correction, `forward(header) ∩ reverse(latch)` includes the whole enclosing SCC. An inner backedge consequently expires an owner introduced before the inner loop. Removing either feedback edge from the reduced program made it pass. The independent seven-block graph oracle uses no checker/validator state.

LLVM reference3623fe661ae35c6c80ac221f14d85be76aa870f1, `llvm/include/llvm/Support/GenericLoopInfoImpl.h:459–485,578–596`, identifies feedback whose header dominates its latch and walks predecessors backward, stopping at the header. Hirsel uses its existing dominator/SCC algorithms and that traversal. Its explicit deviation is to retain the old SCC region for every feedback edge in an SCC without a single dominating header, including natural subloops inside an irreducible component.

Tests independently assert inner-loop membership, enclosing-backedge expiration, outer continue/break behavior and irreducible fallback. Driver controls end the real owner named by a branded token in nested, inner-owner, outer-break and outer-continue shapes; each forged graph rejects InvalidOrigin. Ending a copied call argument was an inadequate initial mutation and is retained only as diagnostic evidence. The existing outer-loop forged fresh-rebinding rejection also passes.

## Before and after evidence

Before implementation, tar truncation27 and its reduction rejected InvalidProjection at places46,47,65–68; the frozen std source49b749429bfff937c183b0d277923a9d899f0f48 reproduces it. The11-line joined-loop program rejected ExpiredLoan1/2 and MissingLoan. wc2 initially rejected PROV-1; producer renewal exposed the historical InvalidOrigin at its mutable File argument.

After implementation:

- All33original affected tar programs execute successfully interpreted and natively. Native evidence is nine explicit four-case smoke batches; the initial batch's joined-view failure was separately fixed and retried. All33tar sources/assertions are byte-identical to baseline. Their33obsolete known-failure rows are removed.
- Joined-view smoke passes4configurations, real-Io replay, Cranelift and runner/executable comparison (11image runs); separate Cranelift parity passes1/1. The original joined source also runs interpreted. The augmented driver control passes native build/stage validation and forged-loan rejection.
- The buffered-files regression executes two files and four lines, passes native smoke and separate parity, and has a matched std manifest. The exact original wc2 source and restored application execute under interpreter, LLVM and Cranelift:6CLI runs asserted by one integration test at22:38:28. Each fixture has a private XDG_CACHE_HOME and uses the production object-store path.
- The restored wc2 package test asserts4interpreted and4LLVM test bodies, and passes. An earlier independent CLI package run also executed4/4interpreted bodies.
- Post-V compiler SHA256d25c7d3456819addd7064e53bd41d9a30934b1ade53cdd9cded5e5397d33ca97 exports both the exact original wc source and the no-std minimal source successfully. Binary/image identities after automatic image refresh are in `post-v/sha256-after`; earlier image bytes are not described as final.

The wc minimal accepted twin is `PROV-1-nested-loop-owner-ok.hir`. Its core is an outer `for`, a fresh R and branded mutable token in that body, and an inner `while let Some(line) = read(r, mut token)?`. New rejected twins cover inner-owner escape and invalidated brands across outer break/continue. The four earlier joined/loop escape and move twins retain their expected rejections.

## Counts and inherited failures

Scoped unit results: lower18/18; V34/34 before the precision correction, then37/37 with its3new graph tests; RC9/9. Authority27passed/1inherited corpus failure and ownership34passed/1inherited corpus failure match existing ledger rows. Final producer controls passed4/4; augmented native joined-view control passed1/1; real-owner expiry control passed1/1 (four accepted/forged pairs); existing outer-expiry control passed1/1.

Initial explicit conformance selection:63cases,60passed/3failed. Two new fixtures initially had invalid Unit entry results, were corrected, and passed2/2 on exact retry. The remaining refreshed-owner failure reproduces with the baseline compiler and stays ledgered. Thus62distinct cases in that selection pass; no repeated green execution is implied.

The authorized rejection sweep selected all1438current `-bad.hir` cases plus2positives. Conformance validated1426:1421expected outcomes passed and5inherited diagnostic mismatches remained. Those5still reject (3source diagnostic mismatches and2Vconstructor-field rejections for inherited missing J diagnostics). Fourteen cases lack required twin headers and were not executed by conformance; the post-V CLI independently checked and rejected all14. Therefore all1438bad programs remain rejected, while the conformance gate itself is explicitly red for inherited metadata/diagnostic issues. All are ledgered; the2missing-J cases have new per-case rows in addition to the existing ownership corpus row.

Zero-body CLI tar `test` runs and an over-filtered RC target are setup evidence only. The initial53case native smoke timed out; a subsequent broad Cranelift selection and a queued speculative transport experiment were cancelled. No successful execution count is inferred from those attempts. Failed fixture setup attempts (missing std manifest, compile-input inventory, package identifier, object-cache environment) are retained separately; only corrected executions are counted.

Native batch8's remote executable began22:18:30 before the V edit was detected22:21:18, so it is pre-V evidence. Batch9 built the V correction separately. Source certificates and `native-batch-source-intervals.md` pin these epochs; earlier successful cases are not claimed to have consumed later source edits. No performance or adoption gate was requested or measured.
