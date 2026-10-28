# Scoped callable transport, stage 1

The source language and heap-backed callable representation are unchanged. TIR 9
records `Body.scoped_parameters` and the existing four-way `CaptureContext` on each
closure creation. Compiler ABI 4 appends nonretaining parameter and capture field
selectors plus their digest to `FnAbi`. LIR 50 records scoped closure sites;
validator 15 checks them. Runtime ABI 38 is unchanged. Export v9 and all content
keys bind the metadata; absent required fields and stale versions fail import.

V derives formal selectors from the original declaration before substitution and
checks nested declarations, capture modes, existing result relations, authority,
multiplicity and all exits. Its separate finite-union solver tracks source
retention origins through local contents and joins. It checks owning/task captures
at construction, even when dropped before return, mutable hand-back, returns and
may-retain calls, plus instruction-failure and terminal cleanup sites. Empty
provenance is sparse; a definition with no scoped roots needs no provenance pass.
Existing V loan, result, authority and cleanup replay remains authoritative for
referents and effects.

C seeds every selected formal/capture even when its physical holding is ordinary
heap-owned. It reconstructs reaching slot versions independently over normal and
instruction-failure edges, then unions scoped origins, environment references and
existing referent loans separately. Reads, copies, stores/replacements (including
displaced contents), field construction/projection, weakening, recasting, nested
captures and block arguments preserve provenance. Unobserved paths collapse to
the whole carrier, so cycles have a finite vocabulary. Code identity does not
clear provenance. Owning copies, heap carriers, owning captures, results/errors,
persistent/mutable hand-back and unqualified operations reject scoped origins.
A borrowed nested capture needs its child capture contract; a forwarding call
needs its nonretaining parameter contract. Linked direct/bound calls and closure
entries must agree with the actual body's contract. Every selected body still
runs ordinary C. Unknown external/operation contracts grant no forwarding right.

Ordinary Borrow Callable → Address → Copy → Return keeps its ownership meaning
and passes C. The identical body with an explicit nonretaining selector fails.
Owning source callbacks, including taken callback returns and generic owning
values, remain legal. `OwningScoped` stays distinct from `Scoped` and receives no
new optimization permission.

The analysis starts from Hirsel's finite-union aliasing implementation
(`crates/hirsel-lir/src/validate/aliasing.rs:69` at
`6079c0d2e44e7ed0f934b55fde139fc5d5744ffe`). Ordered/transitive capture propagation
follows Goose `8511ae34bdc9aabf77baaab80b4d659ea87835e2`,
`src/codegen_frames.h:115,210`, and its nested/repeated/recursive capture vectors.
Hirsel deliberately retains owning, erased and runtime-selected callable values,
independent V/C/H and bounded specialization; Goose's static-only function-value
restrictions do not apply.

These contracts prove no frame extent or allocation saving. Environment-address
sets describe current heap environments, not activation/generation tokens. Source
capture-to-ABI mapping conservatively marks whole capture carriers and does not
provide arbitrary symbolic field precision. Dynamic projections conservatively
retain provenance. Partial initialization, captured resource/Shared/authority
owners and failure order continue to use the existing validators and H ledgers.
Unknown cases cannot justify future placement. Stage 2 needs checked direct
capture elimination; stage 3 needs distinct data-pointer entries and owning
adapters, frame begin/end and generation proofs, tail/reentrant/loop checks,
independent H stale-generation detection and both backend implementations. The
CLO-1 per-site zero-environment allocation/RC gate remains open.

## Qualification boundary

A per-unit C receipt checks symbolic scoped obligations; it is not a complete
module nonretention certificate. `qualify_scoped_transport` independently runs C
for every body and validates linked claims. It refuses unknown indirect entries
whose ABI claims nonretaining arguments or captures: the existing physical
callable layout carries no nested entry-contract evidence. Existing source and
native acceptance keep the safe heap baseline. A future proof consumer must use
this complete qualification boundary; neither a per-unit receipt nor an ABI
checksum authorizes optimization. Representing and independently validating
nested entry identities is required before this indirect subset can qualify.

`check_scoped_transport` is the strict per-unit C proof check and
`qualify_scoped_transport` adds complete-module checking. Ordinary C stage receipts
retain ownership/representation guarantees and bind all metadata, but grant no
scoped nonretention permission. Unsupported generic Borrow forwarding and locally
owning `OwningScoped` environments keep their current safe heap behavior. Strict
qualification still refuses unknown forwarding and ownership-creating transport
of scoped origins. V similarly exposes a separate strict qualification check;
ordinary declaration/classification and existing escape validation remain in force.
The retention-origin solver runs at explicit qualification and when ordinary V
rechecks an inferred nonretention claim. A private borrowed-data helper summary
qualifies forwarding only when its original
parameter graph has no unresolved type parameter or callable node. Specialization
does not supply a missing nested callable contract. No generic declaration acquires
a nonretaining promise through specialization. Private inference excludes those
parameter graphs from its summaries and declines functions with written contracts
until its proof covers the generated clause calls and their
data-parameter entry contracts.
Any future optimization must qualify its supported subset again after every
transformation; receipt identity alone cannot replace that check.

Ordinary native linking checks contract consistency whenever a body is present.
Absent foreign/bodyless entries keep existing heap acceptance and grant no proof;
complete scoped qualification requires every contracted body. This distinction
also applies to unknown indirect entries and generic Borrow forwarding.

## Stage 2 implementation status

Compiler ABI 5, LIR 51 and validator 16 add `FnAbi.scoped_entries` for whole callable parameter selectors and content-named invocation identities. The FnAbi cache digest is v2. TIR/export 9 and runtime ABI 38 remain unchanged. Complete C independently reconstructs nested identities from symbolic declaration parameters, actual linked FunctionRef/ClosureCode and SSA storage/CFG unions, verifies higher-order actuals, and rejects missing/forged/mismatched evidence. Source V checks nested declaration selector correspondence independently. Hidden capture-entry and projected/dynamic evidence remain unsupported strict-proof cases with ordinary heap acceptance.

`scoped_consumer_9` consumes strict V and complete linked C before bounded direct capture specialization and reruns complete C on every proposed site. It passes ordered ordinary arguments then explicit borrowed captures and erases only dead callable carriers to ordinary Unit IR, retaining source loans and captured-owner cleanup. Unsupported use, ambiguous captured data, unrepresented result relations or budget saturation discard the whole site transaction. Owning/once environments remain heap-backed. See [scoped-consumer-9.md](../scoped-consumer-9.md) for exact scope and reference adaptations. This direct subset supersedes the stage-1 cost limitation above; no frame placement or full CLO-1 claim follows.

Production stage-2 elimination is enabled only by explicit release/LTO builds; debug/default builds and interactive native runs keep the heap path. Independent proof reconstruction remains expensive on large packages. Per-compile strict-body memoization binds exact body/input content as well as entry identity, and complete linked checks still rerun for each proposed transaction.
