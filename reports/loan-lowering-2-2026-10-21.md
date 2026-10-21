# Loop and joined-view producer repair

The three historical families were reproduced on origin/main 0a0d9aed835ce1808f18342a139835935187df05 before implementation. The source checkout is isolated; the shared checkout is unchanged. Raw evidence and manifests are retained in `/workspace/code/hirsel-briefs/loan-lowering-2/evidence` and `/tmp/loan-lowering-2`.

## Mechanisms and prior art

Rust reference a7c37f9f0ee5e93396106e9de20f4a86b4154e63:

- `compiler/rustc_mir_build/src/builder/expr/into.rs:23` lowers nested expressions into a receiving destination; lines105–128 join both paths at a block.
- `compiler/rustc_mir_build/src/builder/scope.rs:1383` schedules a value's destruction in its owning scope; `:1470` separates storage end from value destruction.
- `compiler/rustc_mir_build/src/builder/expr/as_place.rs:308` records a field projection with its physical field type.

The producer follows those separations. Hirsel has value-mode and brand loans rather than Rust references: a joined view of global content still requires source access permission until its last use. This is the reason for promoting held access loans even when the type has no locality bound. Physical pattern payloads retain J's substituted identity evidence and the declaration's locality; receiving bindings retain their separately checked receipt types. This uses additive `OwnershipFacts.physical_patterns`, with no TIR/LIR or native ABI change.

Go reference ebe2feefc400f813aefdc8a036e66d2619da6649, `src/archive/tar/reader_test.go:793` (`TestReadTruncation`), is the original source of the tar vectors. All existing truncation, short-refill and GNU-incremental assertions are retained.

Source replay also recreates owners declared inside a loop. Expiration from a previous incarnation must not poison a fresh producer receipt at the same syntax site. Renewal conservatively retains rows held by bindings outside the body and rows referring to external origins; no escape or mutation permission is added.

## Evidence and limits

Before: tar truncation27 and the reduced tar program both fail TIR InvalidProjection at places46,47,65–68. The same truncation27 failure reproduces with the frozen original std source at49b749429bfff937c183b0d277923a9d899f0f48. The11-line nested joined-loop program fails ExpiredLoan1/2 and MissingLoan. wc2 initially reports PROV-1 during source replay; repairing replay exposes its historical InvalidOrigin.

The CLI `test` command on a source whose entry is `main(t: Test)` executes zero `@test` bodies. Those logs are setup/export evidence only. The conformance runner executes those entries and their assertions. Buck `root//:*` is rejected by the current version; target inventories use `root//:`. Selected conformance configuration uses literal paths, split into explicit semicolon-separated slices.

The initial four-slice selection executed63 cases:60 passed,3 failed. All33 original affected tar cases passed. Two new entries had an invalid Unit return contract and were repaired to return Result; the third failure is the existing `PROV-1-refreshed-owner-loop-ok` diagnostic, reproduced unchanged using the frozen baseline compiler and already listed in known-failures.tsv. Later exact retries are recorded separately.

Unit execution: lower18/18 and validate34/34; authority27passed/1existing corpus failure; ownership34passed/1existing corpus failure. The latter failures exactly match the checked-in ledger. Agreement selected24:22passed/2failed, including all3new producer controls; a physical sibling-evidence regression was corrected and retried separately. A pristine0a0d9aed exact two-test control proves the forged-container diagnostic ratchet is inherited (1passed/1failed); its independent validator still rejects the forged graph.

No performance or adoption gate was requested or measured. Native, final exact counts, wc2 disposition and landing identities are recorded below after completion.

The separate wc2 package Rust integration test executed1test and asserted4passed interpreted plus4passed LLVM package bodies. The direct CLI package invocation later independently executed4/4 interpreted tests. Initial native selections include53cases (50tar-read corpus entries plus3producer controls); this is scoped input, not a whole native campaign.

## Remaining nested-loop origin rejection

The original `examples/wc2/repros/invalid-origin.hir` is still open. Neither its source nor the package's existing workaround was changed. After source replay is repaired, TIR validation rejects the mutable File argument with InvalidOrigin. This smaller program, with no std dependency, isolates the remaining condition:

```hir
type R { x: u64 }
fn owner() -> R { R(x: 0u64) }
fn brand(r: R) -> Option[u8 of r] { None }
fn read(r: R, mut token: Option[u8 of r]) -> Result[Option[String], ()] { Ok(None) }
pub fn main() -> Result[(), ()] {
  let ns: List[u64] = [0, 1]
  for n in ns {
    let r = owner()
    let mut token = brand(r)
    while let Some(line) = read(r, mut token)? {}
  }
  Ok(())
}
```

Source checking accepts it; export rejects InvalidOrigin(PlaceId(33)). Removing either feedback edge makes export pass. In `hirsel-validate/src/flow_origins.rs`, loop expiration takes forward reachability from a feedback target intersected with reverse reachability from its source. For an inner loop inside an outer cycle this includes the enclosing strongly connected component, including the owner introduced before the inner loop. The inner backedge therefore expires that still-live owner. An independent seven-block graph oracle confirms the region error; it uses no validator/checker state.

The proposed follow-up is to compute reducible natural loops by walking predecessors from the latch, stopping at the dominating header, and preserve the current conservative behavior for irreducible feedback. This lane changes no validator or native importer rejection guard. Its brief explicitly excludes validator changes; the scope consultation is retained as message583154. The failing buffered-files fixture is preserved with the evidence, outside the passing corpus. No workaround restoration or claim that all three families pass is made.

## Final producer scope

Implementation commits before landing/rebase:117ea76a (joined access permissions), f9ed2e2c (physical payload locality), 0cac4925 (fresh loop-local brand replay), and69d9c8c7 (current lexical item guard). Commit8907adf5 records the independently reproduced forged-container diagnostic mismatch. Final source identities are in `final-compiler-source-certificate.json`; earlier CLI artifacts are separately labelled and are not the final compiler.

The final exact driver selection executed4/4 tests: sibling identity preservation, fresh loop-local owners, nested joined views and physical payload locality (`loan-lowering-2-final-producer-controls.log`,21:33:53). The33obsolete tar InvalidProjection ledger rows are removed because all33original programs executed successfully. The inherited authority/ownership corpus and refreshed-owner failures remain recorded. Native batch and landing results are appended to the external disposition once those commands finish.

## Native joined-view proof transport

Native checking exposed a second stage of the joined-view family: a whole borrowed assignment of an existing joined view was classified as ordinary storage rather than an SSA view. The existing importer rejection for a borrowed read of counted storage outside any loan correctly refused that representation. After coordination with the borrowed-encoding lane (messages583776,583826,583833), `view_assignments` records whole forwarding separately. The alias analysis follows it only to a proven alias; joined-view propagation follows it only to an eligible joined view. Forwarding adds no loan, takes no address of an SSA view and preserves its existing holding. C/H and the ordinary-storage guard remain unchanged. This extends the current native proof representation rather than adding a TIR/LIR construct.

The joined fixture's first native smoke and parity runs failed at the guard. After proof transport, native smoke passed all4configurations, real-Io replay, Cranelift and runner/executable comparisons (11image runs), and the separate Cranelift parity selection passed1/1 (`loan-lowering-2-native-joined-forward.log`,21:52:17). The augmented driver regression forwards through another match, reuses the source view and strips the original loan evidence in a forged graph to require continued importer rejection. The speculative transport-usage change was cancelled before execution and fully reverted.

The corrected two conformance entries executed2/2 successfully at21:43:38. Across the original63case selection and those exact repairs,62distinct cases pass and the inherited refreshed-owner case remains red. All4new rejected twins retain their expected diagnostics. The initial53case smoke selection timed out remotely; no execution count is inferred from that attempt. The subsequent broad Cranelift selection was cancelled and contributes no pass count. Native tar evidence comes from explicit four-case batches.
