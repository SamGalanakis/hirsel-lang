# hirsel-validate

The IR checker (V): independent static validation of TIR before publication and
execution. It reports compiler invariant failures, never source diagnostics, and
covers structure, types, CFG, live slots, loans and cleanup dataflow.

Allowed dependencies: the pure hirsel-graph utility, hirsel-tir, hirsel-types, hirsel-syntax, hirsel-intern; no
implementation crates. hirsel-lir, hirsel-rc, hirsel-heapcheck and hirsel-llvm
consume validated TIR but never depend on this crate, so a checker edit rebuilds
only its users: hirsel-interp (the structural entry assertion), the driver
(publication) and hirsel-lower's tests.

Tests build TIR by hand. `tests/it` runs them through the public API
(`checks.rs` and its `*_controls` and `review` modules, with fixture builders in
`support.rs`). `callable_promise_controls.rs` covers written callable state
promises and result capture selections through local, tuple and replacement
storage, with accepted twins. `gap_controls.rs` and `typing_gap_controls.rs`
hold the only guards of rules no conformance file reaches (J rejects every
source form first): aliased arguments, unsupported cleanup, reachable
`Unreachable`, orphan bodies, entry captures, finalizer signatures, each
structural arena arm, match-table typing and coverage, tail witnesses and the
remaining typing labels, each beside its accepted twin. These paths reuse the shared type comparator used
for call arguments and constructor fields. `src/internal_tests.rs` keeps the eight
tests that drive private internals; it reuses `tests/it/support.rs` by path. The cleanup replay counter
(`ValidateCleanupReplaySteps`) counts in every build so that the integration
tests can read it.

## Static validation

`hirsel_validate::validate(&Definition, &Signatures) -> Validation` is pure: signatures
are an explicit `BTreeMap<DefPath, Signature>` owning complete type snapshots. It
never queries a provider or executes code. Include the owner's signature when it
has parameters, plus called and constructed nominal signatures. Missing facts
fail closed. Call and owner origins are proved from scoped identities and
executable producer edges. Owner-view opaque representations independently prove their declared structural
bounds and that field authority fits the export, including declarations unused by
any body. Public opaque snapshots have no representation to inspect. Flat owner
layouts reject capability, handle, callable and resource fields, including nominal
and generic nesting. Represented capability declarations independently prove their exact nominal
summary, owner-only implementation ceiling, structural bounds and global scope
retention. Global/scoped kind supplies no copying or cleanup proof. Source-less
intrinsic/FFI declarations retain explicit trust.
The report contains only `violations`. `hirsel_validate::assert_valid`
panics for violations and returns unit; it is the helper for lowering's tests.
The driver's mode-before-lowering and publication guards remain its responsibility.

`ValidatedSignatures::new(Signatures)` owns and independently checks a complete
immutable signature input once. Its `validate` method retains structural/type-graph
error precedence and the same signature violations as `validate`. Its
`validate_with_signature` method shadows one materialized owner through a borrowed
view, checking that replacement and retaining all other signature errors in path
order. `work::read(Counter::ValidateSignatureRows)` counts actual signature checks for
scaling regressions.

`Violation` identifies an optional body, block and instruction; a block with no
instruction identifies its terminator. `ViolationKind` contains compiler-invariant
errors, **no RuleIds** and no source diagnostic wrapper. Results are deterministic
in arena/program order, with structural errors first. Type errors prevent unsafe
dataflow throughout the definition, including its closure producers. Fixed-point iterations emit nothing; the final states are
checked once. Multiple independent errors, such as a mismatched cleanup and the
resulting leaked slot, are retained.

| Check | Violations |
|---|---|
| Definition/body/place/local/scope/rvalue/cleanup/loan references and type provenance; capture storage only in closure bodies | `InvalidId`, `InvalidType`, `Duplicate`, `WrongKind`, `ScopeCycle` |
| Type graph integrity, builtin type/size argument arity, recovery rows and cycles | `InvalidTypeGraph`, `InvalidSignature` |
| CFG reachability, closure/contract body roots and dangling rvalues | `UnreachableBlock`, `UnreferencedBody`, `UnreferencedRvalue`, `ReachableUnreachable` |
| Every constant, assignment, snapshot, unary/binary operator, construction, closure, shape, condition, return and call | `TypeMismatch`, `InvalidOperator`, `InvalidConstructor`, `InvalidProjection`, `Arity`, `MissingSignature` |
| Call/constructor ordinals, generic kind/arity, independent type stores | `Duplicate`, `InvalidId`, `TypeMismatch`; calls use shared `import_type`/`substitute_type` and `semantic_equal` |
| Variant/bool CFG switches | `MatchCoverage`; unique in-range arms plus an explicit default cover remaining cases, or every case must have an arm before an `Unreachable` default |
| Definite initialization, fields/partial moves, replacements and borrowed storage | `ReadBeforeInit`, `UseAfterMove`, `DoubleMove`, `WriteToLive`, `BorrowedMove`, `InvalidUse` |
| Loan lifetime, derived borrowed temporaries/captures, source access, mutable exclusivity and aliased mutable call arguments | `MissingLoan`, `LoanConflict`, `LoanOutlivesSource`, `InactiveLoan`, `ExpiredLoan`, `LiveLoanAtExit`, `AliasedArguments` |
| Exact failure/normal-exit release set and conditional drops after CFG joins | `CleanupMismatch`, `LeakedSlot` |
| Cleanup releases exited scopes innermost first, each scope's temporaries before its locals, reverse evaluation and declaration order | `CleanupOrder` |
| Cleanup without sufficient frozen-IR ownership facts | `UnsupportedCleanup` |

The CFG analysis joins finite sets of uninitialized/live/moved states, possible
loan activity and temporary evaluation precedence. It uses a worklist to a fixed
point, including loops with multiple entries. It checks availability on **every**
path and allows conditional drops for slots live on only some predecessors.
Fields are tracked separately; whole-value reads require their tracked parts.
Loan overlap distinguishes fields and treats all elements of one sequence as one
storage. Explicit borrowed/mut parameters are caller-owned. Resource finalizer owners use
`SignatureKind::Type`: DROP-3 self is borrowed and the result must be unit. Box
payloads project through `Field(0)`. Plain inline bits
(including structural aggregates of bits) need no release under DROP-1/MEM-1;
optional scalar drop actions are allowed. Transparent nominal plainness and copyability
are checked from the declared representation after substituting actual type arguments;
opaque values require exported bounds. Recursive nominal instantiations retain their
arguments, and resource/once/iterator payloads cannot gain a copy proof. Strings, bytes, counted storage and
resources do need release. Unproven generic/opaque cleanup is conservative.

Failure lists are checked for potentially failing operations before writing
results. Operand extraction/constructor preparation may fail before or after a
Take, so conditional cleanup covers the union of those states. Nonfailing storage/loan bookkeeping
and bit constants only need an in-range failure-list ID. Calls transfer Take
arguments before checking the call's failure list, so earlier argument temporaries
belong in cleanup while preparing a later argument, and must be absent from the live release set after
callee entry. Listed conditional drops of proven-dead storage are legal skips. Return extracts its operand before cleanup. Cleanup continues in
listed order; validation never invokes a destructor.

Limits: raw in-range arena IDs from another owner cannot be identified; TypeIds
are the exception because their provenance is checked before the shared accessor.
TIR cannot prove omitted source effects or preservation of the source's original
evaluation order; lowering tests own that obligation. The structural type walk uses an explicit DFS stack and colors;
acyclic graphs have no separate depth limit, while recovery/cyclic graphs fail
closed as `InvalidTypeGraph`. Nominal
recursion is not a structural cycle. Nonplain field and index projections follow their owner locality where the
declared field is Global (OWN-8), rederiving root sendability through the
shared projection helper; plain fields and nested type evidence remain exact. Equality is normalized rooted equality, not
alias expansion or arbitrary subtyping. Parameter defaults, storage assignments and returns may weaken root value
modes/cleanup promises (MODE-2/7), but cannot grant stronger evidence. Nested
type graphs and placement still require semantic equality. Root brands may
erase or widen into a known storage union. Function values
may satisfy a broader held-authority promise and a weaker `uses` restriction
(CLO-2/4, CAP-2/3); unknown authority cannot prove a known limit, and unrestricted
uses cannot satisfy a restriction. The same relation applies at returns,
assignments, call arguments and constructor fields, and at every depth of the
value in the direction the nested callable is read (`hirsel_types::Variance`):
covariant in parts, sequence elements and results, contravariant in non-`mut`
parameters, exact for a `mut` argument's whole type, its root's callable promise
included (the callee writes it back). Erased captures and cleanup summaries are
received the same way below the root as at it: fewer captures than declared, and
any summary where allocating cleanup is permitted; a constructor field's own
summary stays exact. Shared rooted equality still
decides the remaining callable structure. A `@noalloc` callable also satisfies a
promise permitting allocation; a callable permitting allocation cannot establish
`@noalloc` (CLO-2, MEM-5). Constructor fields
use the same root mode relation, with exact cleanup promises for the declared
field layout. Copy captures may
weaken only root uniqueness from Unique to Aliased, matching the authority
checker's capture type while leaving the source operand's type intact. Aliased
sources cannot establish Unique captures; all other type structure, modes,
brands, placement and cleanup promises remain equal. Taken captures require exact types. Borrowed and mutable capture slots use the
root receiving-value relation: parent flow independently proves their current
uniqueness and crossing promises at creation. Weakening captured uniqueness or
locality rederives sendability; nested types, scoped binder identity, placement,
brands and cleanup remain checked.

`Place.binding` identifies a source name and its relative field path, separately
from storage. `BindOrigin` introduces a fresh binding incarnation or an alias of
an initialized operand. V verifies the source/target storage relation and derives
identities in the existing ownership fixed point. Parameter identities come from
declarations; `ReadConstant` preserves the declaration identity of a materialized
constant. `EndOrigin` represents a written take or replacement, while administrative
`Use(Take)` transports ownership between storage slots. Named accesses must match
the storage and relative projection of their introduction.

Each value retains the identity proof current when it was produced. Calls replay
brands, arena origins and selected actual locality through those proofs. Missing
or ambiguous derivation is `InvalidOrigin` or a type violation. `CallOrigins` has
been removed. Unresolved caller-destination
selectors fail closed until an allocation-site producer supplies a concrete proof.
A declaration selector of an argument's retained brand is distinct from the
identity of that argument's wrapper value.

Mutable calls expire outside holders before independently reinstating the typed
origin proofs of the storage handed back by the callee's exact receiving
contract, on normal and failure outcomes. Each tracked part receives current
declaration-derived brand evidence, so an installed sibling-branded field stays
valid after a whole-record reborrow. Hidden captured environments retain their
separate expiry. Hand-built field replacement controls retain wrong-owner and
expired outside-holder rejection.

Owner end, replacement and mutation expire dependent values. Slots insertion and
removal preserve container identity (MEM-14). DFS feedback edges expire owners
introduced on their cycle, preventing a branded value from surviving into a fresh
iteration. Closure environments retain the identities at creation; validation
propagates them into nested bodies before reporting flow violations. Re-entering
once or many closures cannot introduce a fresh incarnation of an outer capture.

`Body.parameter_scope` identifies its declared value-parameter binder independently
of storage ScopeId, LocalId and BodyId. Owner signatures bind their free references
to this scope before exact comparison. A closure's published own references stay
Bound; its parameter/result comparisons bind them to the nested body's scope.
Captured outer references remain Body references. Wrong scope, ordinal, type,
mode or destination is a violation. `OwnerOrigins` and its origin-erasing
comparison have been removed; call source relationships are checked in flow.
Authority references to checked parameters evaluate against their declared held
authority without changing exported signature summaries.

Region release, arena closing and task
joining need runtime-specific ownership facts absent from this seam and fail
closed as `UnsupportedCleanup`; no RegionId arena is invented.

The interpreter uses **one structural-only debug assertion** at its root entry,
after the existing frame-entry checks preserve their Tool Error.
It needs no signatures and changes nothing in release builds. Full validation is
available through the explicit test helper and publication API, rather than
guessing signatures through `TirProvider`. All validator regression and
mutation tests remain in this crate, with no new dependencies.

Timing on main `fca0c36` with the validator: 31 alternating samples of the same
159-test interpreter suite, compiled binaries run with one test thread (three
warmups). Median without the assertion **273.680 ms**, with it **278.084 ms**,
added **4.404 ms** (1.6%; individual runs 266–322 ms). The earlier 74-test suite
on `b76b147` measured **91.073 ms** without and **92.801 ms** with the assertion:
**1.728 ms** added (1.9%). The full validator/property suite takes about **0.37 s**.
These costs do not justify removing checks from the per-push gate. Release builds
contain no interpreter validation call.

`Rvalue::Unary { op: Unique, value: Copy(..) }` preserves a MODE-2 deep-copy
operation. Validation establishes root Global/Unique evidence and preserves
every nested type, brand, placement and other proof; ordinary `Use(Copy)` still
cannot strengthen these modes. Operand validation independently requires a
copyable source. Already-unique reuse remains an ordinary checked Take.

Generic graph substitution takes the declared `NominalRoles` and normalizes
`FunctionType.error` from its substituted result, including nested callbacks.
The shared `result_error` helper recognizes Result by nominal identity. Pure
imports preserve existing channels; body checking needs no post-substitution repair.

Structural Plain/Copyable/NeedsRelease/Sendable proofs and value compatibility
now come from hirsel-types, using the supplied declaration snapshots. Projection
locality changes rederive root sendability; nonplain Local parts cannot retain
a global Sendable promise. Unique extraction and read-only comparison likewise
rederive only the root summary after their root uniqueness judgment; a
comparison also sets aside nested `unique` parts, since it only borrows. Other
nested proofs remain exact. Generic expected graphs specialize derived summaries;
comparisons never normalize away missing actual evidence. Root brand erasure
and membership in a joined storage brand are permitted without granting a new
origin; branded source-use diagnostics remain J's responsibility.

`Rvalue::Size(SizeParameterRef)` reads a generic size as u64. Validation proves
its exact declaring scope, ordinal and parameter kind against the owner signature
and typed closure edges. Every closure can refer to outer binders explicitly;
its own generic references name a separate scope. Forwarded symbolic size call
arguments require the same proof. No local-slot ordinal or unchecked size-binding snapshot substitutes
for a declared generic parameter.

Local result returns receive a CFG source-inclusion proof against scoped local(x)
parameters (`InvalidResultSource` on failure). Borrowed parameter roots and derived
source sets are independent of explicit loan IDs. Calls retain only the result
sources declared by parameter ordinal; their other argument loans do not become
result dependencies. Nested type contracts remain exact.

Branch values retain the union of their possible source loan IDs. Both instruction
and cleanup `EndLoan { loan, if_active }` distinguish a definite end from an end
of a branch alternative. A strict end still rejects an absent loan. Expiration is
recorded on dependent live values when an active loan ends and joins with those
values; an alternative absent on a path is not an expired selected loan.

`Rvalue::Use(Take)` may relocate a whole borrowed Temporary slot while preserving
its nonowning status and original dependencies. This permission does not apply to
parameters, captures, named locals, projections, calls or constructor operands.
It transports an internal view; it cannot turn a borrow into owned data. Mutation
regressions retain `BorrowedMove` for taking that relocated view into a call.

Generic callable substitution derives builtin handle authority through the shared
nominal role rule. Task carries Scope; other scheduling handles carry Tasks plus
retained payload authority. Claimed callable results cannot erase that authority.

Erased Iter types retain `Type.erased_captures`. Declared `FromArguments` uses
scoped RelationPlace selectors; shared instantiation derives known authority and mode
sendability from selected actuals. None remains unknown. Capture evidence survives
generic/scoped graph transport and v5 fingerprints; revision 6 also permits it on function values.
The validator derives call results independently from their declaration.

Heap keywords lower to the registry-selected private call. Validation checks allocator facts against the canonical Arena role and verifies declared call results. An ordinary Arena-producing call cannot acquire the ambient heap refinement.

Record projections instantiate declared sibling selectors against the checked
owner field path. The field ordinal comes from the declaration, while the binding
owner is checked against its storage introduction by flow. Forging both the type
brand and its access tag cannot substitute a different record's identity.

Constructor fields with sibling selectors are compared in flow, before extracting
any field operand. The shared constructor layout still proves owner, ordinals and
arity statically. Flow derives each supplied field identity, instantiates sibling
selectors and compares complete field types, cleanup and placement. No relationship
is erased or classified as unchecked while awaiting those operands.

Projection layout checking and flow replay share one derivation. Sibling selectors
use the current storage identity of the declared owner field; reads, writes, drops
and binding introductions compare the complete canonical projected type. Binding
locations are flow facts, so a name rebound to another slot cannot authorize access
to its old address. Whole-record moves retain sibling relations under the new owner.
A written take ends the taken name and its owned identities for every holder
outside its storage; parts inside it stop retaining them, since a field read
through that name recorded the relation the move keeps (PROV-5).

BindFields introduces the direct field names of a staging aggregate simultaneously.
V derives their identities from source ownership and complete declared projections;
owned fields introduce fresh names, borrowed fields retain source identities. This
proves destructuring rebranding even when a sibling owner appears later. Ordinary
operand transfers follow the introduction; no producer-supplied final brand is trusted.

Named function value identity is derived only from an exact capture-free forwarding
body: parameter-preserving direct call and immediate return, with storage/loan
bookkeeping only. A changed callee or argument mapping cannot claim the old DefPath.

Borrowed closure captures retain per-field dependency/expiration summaries in the
existing value-flow state. Construction and transfer preserve them; projection
selects a subtree. Statically distinguished paths bound the finite summary domain,
with deeper recursive captures joined at a represented prefix. Copy/take captures
own their identity scope: actual creation operands seed the nested body's brand
proofs, while ending the caller binding does not expire the owned environment.
Capturing a value that itself contains borrowed state preserves that state.
Generic local results retain selected source loans; ending one before invocation
is rejected. Callable signatures need no hidden caller-origin summary (#39).

Mutable alias accesses carry `Place.loan`. A reborrow's BeginLoan source names
its parent access; validation checks active ancestry, exact source containment
(including the evaluated index local), and mutable permission. Reads and
replacement through an alias exclude only its own loan and ancestors from
conflicts. Take excludes none. Live child loans block parent access and parent
end. Named bindings retain their required access loan alongside their current
storage location, so dropping the tag cannot extend an alias's lifetime.
Projection, binding and direct operand producers preserve this tag. Borrowed
temporary value transport retains its selected source dependencies; it does not
borrow the administrative slot. Runtime storage addressing is unchanged.

Dependent local(x) return comparison derives the expected root sendability after
resolving the access contract to Local. Actual operand evidence is unchanged;
the independent CFG source-inclusion proof still rejects undeclared sources.

Dependent return checking derives expected placement from the body's formal
source types using the same source-instantiation helper as call checking. A
conservative unknown borrowed placement accepts a more precise source placement;
independent CFG source inclusion still rejects returns from unselected parameters.
Actual return rows are never rewritten to repair producer evidence.

Constructor layout and projected field replay derive inherited placement with
`placed_part_type`. Field placement uses the shared Value relation, so Heap
outlives arena requirements; mismatched scoped placements still reject. Exact
cleanup and nested type contracts remain independently checked.

Generic-bound proofs use complete actual values and their scoped references.
Captures, aliases and transfers retain those references directly. The validator
reconstructs accessible binder declarations independently from closure producers;
there is no per-value context inference.
Flow tracks sharing that ends a counted value's uniqueness, and derives crossing
properties from the resulting value rather than its original storage row.

Ordinary variant payload accesses require a matching table arm, switch or construction
proof. These finite facts join across incoming edges, travel through value
transfers, and expire when the carrier is mutated. Different cases overlap for
loans. `Packed` payload accesses retain MEM-13's runtime tag check.

A moved aggregate field is absent from the interpreter's `Option<Value>` field
slots. Dropping the remaining aggregate visits only present fields, so a partial
move followed by whole-aggregate teardown releases each remaining value once.

Indirect calls carry `{ value, arguments, forward_caller }`, including an empty argument list for
nongeneric callables. Their generic instantiation receives the same independent
arity, kind and bound checks as a direct call. Actual operand proofs look up each
reference at its own binder; a nested callee's generics are a fresh binder.

Structural reference checks and strict graph validation enumerate every type edge
through `TypeKind::graph_edges`. Provenance and cycle checks remain here;
the view includes callable edges independently of owned-payload proof policy.

Lexical generic declarations are indexed once from the owner signatures and
closure producers, then restricted to each body's ancestors. Type and flow
checkers share these tables; no operand causes a whole-definition scan. The
additive `work::read(Counter::ValidateDeclarationRows)` thread-local work counter guards that
bound independently of elapsed time.

A `mut` call argument marks its root shared unless the callee declares the parameter
`unique`, preserves its state, or is a `Callee::Definition` declaring
`@preserves_unique` on it (never an indirect callee, whose function value may come
from an unproved closure, nor a foreign one, whose C body proves nothing) with every borrowed argument plain or a handle when the
callee is core@1 (bodyless, so it may store a share of one) and its promise does not
list that parameter `unretained`, the argument itself and every
counted taken argument currently unique, and, for a core@1 callee, every other `mut`
argument too; the same promise on a taken
parameter makes the call's destination currently unique. A `mut` use of a part of a
currently unique root is unique. `mutable_storage_exit` requires a `mut x: unique T`
parameter to be unique at each exit and at each evaluation failure edge, but not on
a release's failure edge, where source checking sees no evaluation. A root shared
only by a related call result's counted shares is `borrow_only`. `borrowed_shares` records,
per local, the roots whose such shares its content may hold, through every assignment,
replacement, snapshot and call (a call's result and `mut` arguments receive every
argument's). Before each instruction and terminator, `restore_borrowed` makes a root
unique again once no live local holds one. A release's failure edge is exempt
(`releasing`) only when the release has no user-visible destruction (DROP-1). A failure edge accepts `borrow_only`, and so
does a return whose value holds none (`returning_shares`). A
`@preserves_unique` body is re-proved by a second flow run (`flow_with`) whose entry
makes the selected and taken parameters current-unique, other `mut` parameters as declared; a selected `mut`
parameter is held to the same exits and a selected `take` parameter's returned value
must be unique. Only its "preserved unique
hand-back" violations are kept. A call result declared as a bare type parameter is
expected with the declared root modes (aliased unless written `unique`), whatever
contract argument instantiates it, as the checker publishes it. A parametric call
(`parametric_unique`, from `hirsel_types::parametric_inputs`) makes its destination
current-unique when every argument that may supply its result is. Hand-built controls are in
`tests/it/unique_handback_controls.rs`.

Projected writes affect current uniqueness only when their contents belong to
the parent's counted owned graph. Plain bits and Shared boundaries preserve that
evidence; an aliased counted replacement cannot establish a unique whole read.
Independent fixtures cover all three, separately from permanent storage modes.

Independent validation uses reusable private working type stores for graph
transport, receiving relations and flow mode proofs. Each operation owns a
marked region; leaving it truncates temporary rows and returns capacity for the
next operation. Nested operations obtain another region instead of copying an
active store. No working rows are assigned back into published IR. Scoped
parameter authority inputs are derived once per body and receiving relations
append directly into the caller's region. The copied-row scaling regression
validates many real copy assignments against an owner-size work bound.

`Body.tail_sites` is additive normalization input, transported from
`CheckedBody.tails`. Validation independently checks self identity, exact result
return, absence of post-call observable work and obligations, single-use counted
Data temporaries, and surviving owner/borrow facts from the flow fixed point.
A diverging self call has no continuation: its failure cleanup must hold only
plain, moved or loop-carried obligations.
Plain proofs use V's own nominal signatures and exact scoped declaration table.
`ConstructorHole` is reserved/rejected pending spec issue #46; no optimized CFG
recognizer can establish a missing source tail fact.
Revision 4 stores types once in `Definition.types`; all bodies use IDs in that
graph. Bodies retain cleanup roots and a shared action/parent node arena.

Index on Map/SortedMap denotes the SEM-5 ordered entry position, typed as [key, value] by independent projection derivation. Whole entries and key subtrees cannot be moved, mutably borrowed, replaced or dropped through this access; value fields may be mutated under a loan. Structural container mutation conflicts with the whole-source loop loan.

MatchTable retains an ordered arm list and an acyclic pattern arena of checked
projection places. Match selects the first matching arm from its index; nonzero
starts require that arm's failed guard path. V independently specializes pattern
matrices for exhaustive unguarded coverage (an integer or char column splits its
domain at the bounds of its literal and range patterns; a range's bounds must order
by the place's signedness) and checks binding test/commit modes
against explicit instructions. MAX_MATCH_WORK bounds proof work deterministically;
tooling_limit_error maps exhaustion to an operational tooling failure. The interpreter
uses no expanded decision graph. See the R4 match addendum for ownership, size and
work measurements.

Definition.contracts names capture-free callable bodies with all-Borrow formals
and Boolean results. Their executable producer is unique even when an ensures
clause is called on several normal exits. V independently rejects parameter
consumption/mutation, aliases of that access, captured readonly provenance and
noninert calls. Owned local copies may be consumed or mutated. Clause purity
extends to nested executable closures, without making unrelated frames readonly.

The ownership solver schedules blocks by CFG reverse postorder, separately for
executable and dead-edge audit states. Acyclic predecessors settle before their
joins; cycles retain the same monotone fixed point and report only final states.
`work::read(Counter::ValidateFlowSteps)` counts forward block transfers for
scaling regressions.

Owned Local receiving storage may hold a fresh Global value after a whole
replacement. V derives this current-content lifetime separately from the fixed
storage declaration and must-joins it over live predecessors. Only independently
proved Global content can escape or provide Global declaration-owned field reads;
retained borrowed content remains bounded.

Nested callable allocation and uses promises are checked against their executable
calls, including the effects of borrowed callbacks and scoped indirect callees.
Their producer headers are indexed once per definition; call arguments retain
formal-ordinal lookup independently of written evaluation order.
Canonical flow places are indexed by root and projection. Whole-place access
visits only that root's canonical components, in their original arena order;
source access aliases retain their separate original order and binding tags.
`work::read(Counter::ValidatePlaceRows)` counts actual candidate inspections for scaling tests.

Origin dataflow independently retains field partitions across copies, projected
subrecord transfers, snapshots and closure captures. Writes discard overlapping
partition facts while preserving siblings. Whole and field origin introductions
establish the resulting namespaces; call selectors are instantiated from the
storage partition rather than appending every field to the whole identity.
Assignment failure unions are constructed only for operations already classified
as potentially failing. Constants and unindexed Use assignments avoid unused
whole-state snapshots and joins. `work::read(Counter::ValidateFailureStateRows)` counts the
place rows in requested failure snapshots; it stays zero for plain copy chains.

Materialized Map and Set constructors retain insertion-ordered entry/element
ordinals. V independently proves Key/Data bounds and recovers immutable literal
keys through an interned structural graph to reject duplicates. Unknown or
mutated producers cannot certify literal keys. Nested Map equality preserves
entry order; nested Set equality ignores order. Map entry layout is shared with
indexed projections, with separately derived canonical tuple rows.
Backward result-read permissions use a CFG postorder worklist, including unreachable
components, rather than cloning and sweeping every block for each round. Cycles
retain the same least fixed point. `work::read(Counter::ValidateResultDemandSteps)` counts actual
backward block transfers for source-chain scaling regressions.

Replace.if_live is reserved for mixed live/dead receiving storage. V independently
checks the canonical flag, marks displaced storage live only on live alternatives,
and requires the immediately following conditional release to name that displaced
slot. Definitely live storage retains strict exchange; dead storage initializes.
Closure `uses` checking traces executable capability operands to permitted captures,
frame parameters and in-frame call results. Projected values and replacement
transport preserve that proof; live control joins require it on every alternative.
Capability attenuation does not enlarge the incoming capture contract. Callee
ambient requirements and unknown scoped callback effects remain separate checks
(CAP-2/CLO-2, spec #58). Private declaration and place evidence is indexed once.
Private ownership-state snapshots use `hirsel_types::rows::Rows`, a shared pure container. Joins visit changed branches
and share disjoint union facts, preserving the existing live-path ownership,
current lifetime and field-partition lattices. Origin bindings and locations use
shared key numbering with radix facts; iteration remains key ordered. The additive
`work::read(Counter::ValidateCopiedRows)` counter counts actual copied leaf/collection rows
for the source-level snapshot regression. Published IR is unchanged.


Owner-approved R4 follow-up removes ReadConstant's operand: named reads derive
constant type/identity from the signature and construct nothing in the reading
frame. Shared Materialized/ValueNode/ValueKind rows belong here; TirProvider must
serve constant snapshots and dependency-first/source-ordered definitions (#63).
FunctionReference names a callable and its generic arguments; V checks its
signature-derived value promises directly. Capture-free forwarding bodies and
structural recognition of them are removed; literal/capturing closures stay Closure.

Closure invocation allocation is independently checked for String/Bytes, boxed,
sequence, Map/Set and owned closure constructions. V computes construction escape
through assignment/aggregate/capture dependencies, return/replacement sites and
complete named or indirect call signatures. Private per-body caches retain the
escape fixed point and fixed-size shape proofs. A Plain operand owns no block,
so moving, copying, returning or passing one (a `u64` read out of a local list)
carries no escape. Heap/caller destinations count;
arena/region destinations do not. ReadConstant and FunctionReference construct
no counted storage. Invocation and cleanup obligations remain separate.
Projected unique receipts require a take from a currently deep-unique root, or
a declared unique payload of an unshared owned call receipt. The receipt fact
is separate from deep uniqueness: moving through locals preserves it, sharing
ends it, and unqualified counted payloads gain no uniqueness. Shared is not
traversable by a physical projection. Flat copies retain their independent copy
proof. Pattern physical types and receiving local types may therefore differ
in modes while retaining their checked shape. Hand-built tuple controls cover
fresh, copied and parameter roots. Unique receipts recompute their crossing
proof, and projected handles retain independently derived structural sendability
even when their physical row underclaims it. Local and Receiver controls reject.

Loan expiry follows the installed content rather than the displaced value's
release. Independent flow tests replace a retained result with fresh content and
with another result using the same loan: only the fresh replacement allows the
loan to end before a subsequent receiver read. A forged early end on the latter
path reports `ExpiredLoan` through the existing dependency proof.
`BeginLoan.if_inactive` is valid only for a mixed incoming read permission at
its exact source. The independent loan solver rejects definitely inactive flags,
mutable flags, strict begins that encounter an active permission, and duplicate
source definitions. Native import applies the same proof to transported flags.

Executable producers, definition-wide environment paths and origin selectors now
form one immutable private input shared by frame proofs. Producer rows retain the
first callable header for parameter passing, the last header for executable effects
and every capture assignment in source/arena order. The old header-only table and
per-frame whole-definition walks are removed. Body-specific release, authority,
origin and allocation proofs keep their existing inputs and fixed points.
`work::read(Counter::ValidateDefinitionInputRows)` counts actual inspected definition rows and
selected producer rows; the cold source-level closure test bounds this work linearly.
Retained loan dependencies and expiration use private per-root content partitions,
separate from access permissions. The validator independently derives constructor
field dependencies and transports partitions through operands and replacement;
ending a displaced field's loan cannot expire an untouched sibling. Unknown call
results retain whole-content dependencies. Indexed writes preserve the summarized
contents of other elements. Shared radix rows retain snapshot sharing, and the
body's finite environment-path universe bounds recursive composition. Failure
cleanup still observes the union before and after operand extraction.
A projected receiver inside an independently owned Local aggregate retains its
projected receiving lifetime. V proves ownership, absence of joined ownership
conflicts and the root's Local storage contract before permitting retained view
content. Caller-borrowed storage does not receive this exemption, and displaced
rows preserve the receiving lifetime rather than claim a stronger Global value.

V indexes captured origin roots once per body in immutable Model inputs. Binding
introduction queries that exact root, retaining the own-expression-place exception
and multiple-alias rule. The per-introduction whole-place walk is removed;
`work::read(Counter::ValidatePlaceRows)` includes indexing and lookup work, with cold lets and
closure fixtures through 256 statements.

V's canonical cleanup sets describe owned release roots and live counted
temporaries in every flow state, including general cleanup. Writes select current
temporaries; exact cleanup selects current release obligations. Root order is
observed once for names and queried through existing temporary precedence rows;
the all-local scans and all-pairs root-order loop are removed. Projected-field
order and failure-proof eligibility keep their existing policy.
`work::read(Counter::ValidateCleanupRootRows)` bounds cold closure work without using elapsed
time; sparse-set boundary and exhaustive small release-order controls remain.

Origin expiry queries primary owner-to-value dependency rows for both retained
brands and hidden environments. Every overwrite and join maintains these rows;
ended paths are grouped by origin root and introductions remove only contained
paths. The former whole-value and whole-ended-set scans are removed. Sibling
paths, already-expired values, finite environment summaries and branch joins
retain their previous policy. `work::read(Counter::ValidateOriginDependencyRows)` bounds actual
expiry candidates in cold branded-capture source fixtures.

NoAlloc callable bodies independently check deep-unique extraction against the
source's owned shape. Plain components and Shared boundaries contain no copied
blocks. Counted descendants may allocate even when discarded or when the source
row claims exclusivity; explicit arena/region placement retains its exemption.
An owned operand whose rows below the root are local to an environment is
received by Global storage at a return, assignment or constructor field when
flow proves its content Global and owned at that site, as a call argument's is.
Readonly arguments and whole taken receptions use one shared view comparison;
owned reception uses Value root policy, retaining placement obligations. Mut
arguments preserve exact nested contracts; copies whose source remains live
preserve them too, apart from nested callable promises.
Replacement derives storage independently and uses the actual extraction mode.

Loop feedback expires each exact origin prefix introduced with Fresh on its cycle.
A fresh whole owner expires every dependent descendant. A fresh field expires
that field, its descendants, and brands naming an ancestor; a disjoint sibling
retains its incarnation. Thus renewing `r.a.n` cannot invalidate a carried brand
of `r.a.other`: neither prefix contains the other. `EndOrigin` independently ends
the canonical owned prefix for writes through names and mutable loans; feedback
expiration does not replace that transfer check. V proves each introduction's
storage, field path and loan permissions before accepting it, so an alias cannot
invent an unrelated owner or renew a captured outer name. If the enclosing owner
is introduced in the cycle, its empty prefix remains in the feedback set and
expires every child even when a field is introduced too.

The direct forged-cycle control compares an acyclic graph with one added
backedge: whole replacement, a prefix write, an owned alias write and a mutable
loan write still reject the old carried handle. The disjoint sibling control
passes. The independent iteration-owner/rebinding control still rejects a handle
when its enclosing owner is introduced in the cycle. These controls exercise
expiry at the first read before the next write, preserving incarnation safety.

The Sendable must fact retains the independent producer fact for Iter and
function-value current content when Local receiving storage masks its crossing
mode (#88). Whole assignments,
replacement and call results install that fact; live joins require it on every
predecessor, and clearing or projected writes discard it. Mutable calls reset it
to the fixed receiving mode. Acquiring independently Global owned content can restore the producer fact without changing its receiving
row. A Local iterator cannot invent Sendable from its capture-authority summary.

The Global must fact is a deep content proof on live owned storage, separate from its
fixed receiving row and its arena/region placement. Validated Global inputs and
call results supply the declared graph promise. Constructors independently require
Global operands for that content proof, including inferred Local receiving parts.
Call source origins follow the independently proved current content. A readonly
call through Global receiving storage can acquire nested Global payload types
from that proof; callable headers and Shared contents are not rewritten.
Parameter comparison first honors an already compatible physical payload contract.
It acquires stronger current content only when required, so an inferred generic
Box with Local payload rows keeps the same identity at `shared.freeze`.
Assign composes all part sources,
content loans and retained closure environments before publishing the fact;
Replace checks the incoming content the same way. A whole Copy/Take cannot regain
Global merely because its source storage was declared Global: retained content
bounds prevent the fact. Projected writes conjoin it with the installed content;
whole writes replace it. Live predecessors must all prove it at joins, including
loop heads. Clearing a whole holder clears the fact. Declared Global storage also
rejects Local content installed by projected Assign independently of Replace.
The forged tuple control refills a declared Local field from a borrowed Local
seed and then moves the whole tuple: the projected installation is rejected and
the whole transfer cannot turn the retained seed into deep Global evidence.


OWN-8 Copy/Take acquisition from a must-owned, deep-Global aggregate receives
Global content even when its physical field contract is Local. The fixed field
row and its placement remain intact; only extraction uses the proved lifetime.
Borrow/Mut accesses retain their physical receiving rows; selected call-source
lifetimes use the separate current-content proof. Independent controls
refill a direct or nested field from a Local seed, write on one branch, read
across loop feedback, and access through Borrow/Mut parameters. Each rejects
an escape after the root proof is weakened; proving only an outer shell cannot
make a nested Local payload Global. Retained sources and loans remain independent
checks, including the declared-Global refill/whole-transfer forgery above.

Lexical scope exits select descendant locals from a private immutable scope
interval index. The selected rows are checked in local arena order, preserving
violation order while avoiding whole-local walks and repeated parent searches.
The bounded nested-block regression counts construction, interval searches,
selected locals and ordering work through successful validated publication.

A global closure's capture may not hold a non-Plain row placed in an Arena or
Region at any depth of the captured value (PROV-4), not only at its root;
callable headers are not entered. `closure capture placement` reports it.

A borrowed or mutable non-plain capture slot is the closure's borrowed view of
its source and is typed Local, as a borrowed parameter is (CLO-1). A closure
body's Local result is local to the environment: the closure's type may promise
a Global result only when every captured source is Global, some capture is
borrowed and no parameter is a borrowed non-plain value; otherwise `closure
result` reports it. A whole replaced Local-receiving slot counts as a Global
source only with parent flow's current-global, owned, conflict-free proof at
creation (`closure capture`). A copy of a Shared root is Global (MEM-11). A
specialised result's sendability is structural, or sendable over a sendable
environment. A local callee with a `uses` restriction is inert in typing and
checked at its source in flow; an unrestricted one has unknown effects only
when it is a scoped parameter, and otherwise the authority its row records.

The mixed-placement join rule holds at every depth (PROV-4). A whole local slot's
unplaced local rows (no placement, local), at the root or any part, receive any
content an owned value holds there (`hirsel_types::adopt_unplaced`); scoped
content that reaches such a row is bounded by the frame in flow
(`hirsel_types::unplaced_scoped`, via `scoped_into_unplaced`).

V retains operational nonregular type-proof exhaustion as
`ViolationKind::TypeProofToolingLimit { limit }`. `tooling_limit_error` translates
that and match work exhaustion at publication; neither is a source rejection.
Snapshot-only proof adapters collect that failure instead of panicking or
publishing their conservative fallback. Regular nominal property graphs have
no size/depth cutoff.

Layout, packed-carrier and cleanup checks likewise have no finite nominal-depth
cutoff. Remaining cleanup memoizes an instantiation together with its ownership
state index, so partially moved fields retain their distinct cleanup evidence.
Only nonregular active polymorphic growth reaches the operational proof budget.

An indirect Local result retains the callable's installed environment dependencies,
not its invocation access chain. Selected argument dependencies and independent
result-source inclusion remain enforced. The hand-built two-loan control ends the
callee access before reading its result, and rejects ending the capture loan there.

`representation::RepresentationRecord` is the owner-produced private
representation (shape, generics, layout promises, finalizer body) that native
layout derivation consumes. Only the driver's owner-only query builds one; it
is never a public signature fact.

Bytes indexing has canonical u8 projection type and is read-only even through owned unique roots. Variant reception independently reconstructs executable active variants before accepting otherwise phantom argument brands. Unrestricted scoped callback invocation requires the exact parameter selected by its owner signature; unknown summaries never widen finite authority. Hand-built controls are in tests/it/wave1_controls.rs.

Nested callable parameters supply their own invocation authority separately from
the capture limit. Whole-value staging preserves that provision for taken once
callbacks; flow independently checks the supplied operands and capture sources.

Inactive variant producers are reconstructed through readonly capture sites and
closed local returns. Phantom result brands need no expired owner replay only
when both the declared and receiving contracts prove absence; active payloads,
root brands, access loans and hidden captured environments retain their checks.

Observed projected types also contribute their direct sibling paths and construction
suffixes to the finite environment universe. A loan installed in an unobserved
sibling stays in that sibling instead of contaminating the observed child. Opaque
representations retain conservative summaries; recursive path closure is absent.

Borrowed `for` protocol returns independently compose owned Data yields through
unnamed owned preparation and cursor holders back to their selected sources
(docs/spec/core-library.md). The exact validated association receipt supplies the holder roles; source
bounds, ownership, expiry and result-relation inclusion remain checked. Named
binding homes and ordinary direct calls keep their bounds under MODE-1; spec
[issue #100](https://github.com/SamGalanakis/hirsel-lang/issues/100) records the
separate transitive-collapse proposal. Hand-built controls reject removal of the
receipt, a different selected source and a borrowed return.

TIR schema 2 adds `Definition.protocol_associations`: exact source declaration
fingerprint and open/next block identities. `src/protocol.rs` independently
checks unused associations, exported signature types, scoped field selectors and
unsupported field-selected or callable creation claims. `src/state_proof.rs`
proves every state promise of the validated body's own callable type, entry or
closure, on its binder-exact field selection, including direct/indirect calls and
normal/error/failure paths.
Whole-input creation envelopes of the body's own callable type, entry or closure,
independently replay actual operand provenance through moves, joins, plain nominal records and declaration-selected factory
results. A matching result type alone supplies no provenance; unselected or
unproved inputs reject. A mutable factory callback relays the exact incoming
state/callback pair, has an inert state with a Sendable receipt, declares
preservation of its whole slot, or has a closed slot with a Sendable receipt.
Call compatibility resolves cleanup conjunctions from exact generic bounds through
nested callback rows; it never reads the Source proof verdict.
Native import retains the same evidence.

Written crossing requirements remain separate from independently replayed current
proofs at every type depth. V tracks owned-part proof loss after projected and
enclosing shares, checks qualified input/result/mutable hand-back receipts, and
rederives generic transport admissibility from the callee signature. Neither a
recorded qualified actual nor a destination's evidence row supplies its own proof.
Closure entries receive independently checked capture proofs. Private environment
dependencies distinguish sharing a function handle from sharing counted results
of invoking it, and travel through signature-selected callable results. A view's
Local access lifetime does not erase its independent environment evidence.

Owned crossing paths retain independently proved immutable callable contents when
their enclosing data owner is shared. Their finite domain includes the longest
callable payload path in the local type graph, so successive destructuring through
new temporary roots preserves that evidence; callable headers are excluded. A
qualified type alone supplies no current fact. Projecting a value preserves the
explicit stable descendants already proved on its owner.

Call-result replay derives an underclaimed root summary after actual result sources
are installed, while retaining stronger validated content promises. An ordinary
generic move need not satisfy its destination's derived conditional summary; it
transports the incoming evidence, and cannot create a borrowed capture obligation.

A checked crossing proof does not erase the frame bound of a dependent share
selected from related taken parameter storage. Independent IR twins accept the
related result and reject a forged Global result, even when its source content
is proved Global.
A fresh operand whose complete access lifetime is Global keeps that independent
lifetime; replaying its old parameter storage must not add a frame bound.

Callable headers may retain a known enclosing parameter declaration without
capturing its value. V canonicalizes those header names as declaration constants,
retains transported capture proofs, and rejects unproved data origins and unknown
declaration ordinals. Header names provide no current-content or crossing proof.

Captured views also retain declared arena names referenced by their placement
bounds, even when the arena value is not captured. Arena placement is excluded
from retained data-origin proofs; naming it remains contract metadata and grants
neither data identity nor global or crossing evidence.

Revision 7 transports seal ceilings and schema opt-in through shared TypeDefinition owner/public signatures, not a new instruction or FailureKind. Serializable protocol/declaration proofs independently validate transparent data schemas, including unused opted-in signatures. Runtime Blocked, launch admission and durable recovery are observation/tool contracts; they do not add source effects.

Sealed resource finalizers independently audit calls and allocation against the
owner ceiling and declared destructor promise; public value authority stays nominal.

Type-origin inputs are memoized privately per immutable definition type and body,
shared across the fixed point and reporting pass. Cached may-hold-origin flags
exclude origin-free subtrees; brand and arena-place sites retain lexical binder
depths. Flow identities and expiry remain current-state proofs. The existing
ValidateOriginDependencyRows counter includes actual type and origin inspections;
the ordinary std empty-main regression bounds these without a timing threshold.

`src/serialization.rs` independently proves SER-1 for every opted-in
signature (including unused declarations) and generic use. It opens transparent
representations and checks brands, authority, opacity, refinements, resources and
the admitted constructor list without consuming Source schema descriptors.
Regular recursion uses declaration applications; nonregular expansion exhaustion
remains operational. Hand-built unused signature and generic controls accompany
Source accept/reject twins.

Call validation replays every declared schema constraint through `schema_failure`,
including the integer/float domains of `float.from_int`, alongside generic bounds.
Hand-built domain controls accept the same invalid call only when its rule is removed.

Closure parameter origin proofs are rebuilt after installing capture identities and
header names. A captured owner already present in the entry map still supplies the
parameter proof; a same-typed capture from another owner cannot claim that identity.

SEM-3 format associations independently imply the same uniqueness promise for
the formatting function's second parameter. V reconstructs the association from
the receiver and nominal signature snapshots, both at direct calls and when
re-proving the function body; no source attribute or producer fact is trusted.

FunctionReference derives its error channel from the substituted result, including
an abstract generic result instantiated as Result. Its callable contract is still
replayed independently from the declaration and actual arguments.

V independently derives `Definition.caller_bodies` from its owner declaration and
the entry's contract producers, and refuses caller tracking on type and extern
declarations. Hand-built controls in `declaration_controls.rs` cover mismatches
and accepted twins; `typing_gap_controls.rs` keeps nested contracts untracked.
