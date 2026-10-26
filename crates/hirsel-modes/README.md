# hirsel-modes

The mode checks: ownership, locality, uniqueness, sendability, authority and brands. Two
passes check them, each in its own crate: the authority, provenance and capture check
([hirsel-authority](../hirsel-authority/README.md)) and the flow-sensitive ownership check
([hirsel-ownership](../hirsel-ownership/README.md)). This crate holds what both share: the causal
explanation vocabulary (`src/explain.rs`) and the expression walk. This README documents
all three.

Allowed dependencies: hirsel-span, hirsel-syntax, hirsel-types, hirsel-diag, hirsel-intern;
the two pass crates also depend on hirsel-modes.

Invariants and boundary:

- Consumes syntax/item, CheckedBody and SignatureProvider; produces enriched CheckedBody or diagnostics.
- No checker internals: semantic decisions live in shared tables (MODE-7).
- No storage overlaps for mut; counted task captures cannot borrow; shared roots bound deep uniqueness (OWN-3/6, MODE-2).
- Keep brands and placement separate; substitute parameter origins at calls; join brands explicitly (PROV-1/2).
- Authority includes captures and drops; unknown authority cannot satisfy uses {} (CAP-2/3).
- CONC-2 obligations: a call returning a `Task` or `TaskSet` creates one, passing it `take` to a
  parameter declared `Task` or `TaskSet` moves it to the callee, whose taking parameter holds it
  in the body; a call reported invalid drops its arguments' obligations (section 23).
- Lowering uses mode-checked UseKinds and origins; it may compute CFG liveness but must not change acceptance.
- Diagnostics name causes (section 23): `explain.rs` holds the closed cause and
  requirement kinds, their wording, the one qualifier spelling function and the
  verified repair table. The ownership pass records pending explanations while it
  emits. A temporary cause is the issue site of the loan the value holds on that
  scope; only when a graph failed does it rerun its solve with a trace transfer that
  logs moves and field writes (`hirsel-ownership/src/flow/causes.rs`). The authority walk
  renders at each site (`hirsel-authority/src/walk/causes.rs`). Explanations never change a
  verdict, a rule or a primary span.

## Ownership and locality

`hirsel_ownership::check_ownership_with_clauses(tree, item, signature, &mut checked_body,
captures, clauses, &mut results, provider)`
returns `Result<QueryOutput<OwnershipFacts>, QueryError>`. Call argument requirements
and declaration ordinals come exclusively from `CheckedBody.calls`, in source order.
No body or name-lookup query is made. Every attempted signature read, including a
failed read, is retained in `checked_body.resolution.reads`; successful dependencies
are returned in canonical definition-path order.

The pass updates expression, binding, pattern and existing capture use decisions.
Refined types are appended as immutable rows and table roots are redirected;
declared signatures remain separate. Binding identities are indices of the
scoped binding table, so nested parameter ordinals cannot alias top-level ones.
Projection steps carry H-checked declaration field and variant ordinals,
independent of interning order. Every index of one sequence aliases
for exclusive calls; distinct fields do not, and a parent overlaps its children.
A `mut` call argument reserves its place while the later arguments are evaluated:
they may read it but not write, move or reserve it again. Its exclusive loan, and
the check that no live value still borrows the place, start at callee entry, so
a view whose last use is a later argument of the same call has already ended
(LANG-3, the two-phase borrow of Rust's `v.push(v.len())`). For the same reason a
lent argument's uniqueness and crossing proof, which parameter requirements, the
parametric rule and `@preserves_unique` read, are its place's state at callee
entry, after every argument (`entry_value`), not its snapshot from evaluation.
A borrowed loop over a place (`for x in xs`, any collection) lends that place
from loop entry while it may advance again (SEM-4): a write, move or `mut`
loan of an overlapping place in the body conflicts whether or not the body uses
the element, and the diagnostic names the loop as the cause. A write followed by
`break` or `return` does not, since the loop never reads the source again.
Its element is a place of the source (`element_view`): a copy of it shares a part of
the source and ends the source's uniqueness (MODE-2), as a copy of `xs[i]` does, while
a borrow of it keeps the source unique. A mutable loop's element (`ElementPlace`)
names its source binding the same way: copying it, or storing an unsendable value
into it, shares a part of that binding and of every enclosing loop's owner
(`for x in mut xs { for y in mut x { .. } }`).
A block whose value projects a counted part of a binding the block declares
(`{ let v = ..; v[0] }`) only borrows that part, and nothing moves a part out,
so a consumer that borrows the block's value would read storage the block has
ended: MODE-1 at the part, naming the block's end and suggesting `copy`
(LANG-11, #50). An owning consumer shares the part instead.
Relations compose (MODE-1): a borrowed argument naming a local binding of the body
whose value is related to other places (it reaches them through read loans and holds
no source bound of its own, unlike an owned copy) lends a related `data` result those
places, not its own scope or a read of itself. `OwnershipFacts.composed_arguments`
lists those arguments; lowering lends the result the binding's content loans.

Expression use decisions are published directly on `CheckedBody.expressions`.
`OwnershipFacts.call_arguments` separately records `(call ExprId, argument ExprId,
UseKind)` for each ordinary call operand: source use evaluates a value, while
consumer use grants Borrow, Copy, Take or Mut. `unique` records extraction actions;
`mutations` records proven in-place, counted-detach and arena-detach decisions.
Internal flow states and result-loan evidence are private to the pass; publication
retains only judgments consumed by the pipeline.
Lowering can compute CFG liveness and ordered cleanup from
these facts but must preserve the use and mode judgments. Taking an argument
clears its source during evaluation, including paths exiting through `?` before
callee entry; replacement initialises storage again. Resource takes use the same
move facts and do not request a drop at the take site.

The private CFG builder, temporary liveness and forward fixed-point solver all
use explicit worklists. Backward temporary demand starts in CFG postorder,
so acyclic successors settle before their predecessors. `work::read(Counter::
OwnershipLivenessSteps)` counts node transfers; cycles still iterate to convergence. Recovery types suppress dependent ownership diagnostics.
Diagnostics follow docs/spec/diagnostics.md source/site/rule ordering and prerequisite suppression.
Capture layout is supplied by the separate authority pass: ownership refines the
creation uses and checks mode/access constraints of the supplied captures.
Authority types a borrowed or mutable non-plain capture as a Local view of its
source, inside the body as well as on the capture row, so the body knows what a
function body knows of a borrowed parameter (CLO-1). An inferred Local result is
local to the environment: the frame result keeps it, and the closure's function
type publishes it Global when every captured source is Global, some capture is
borrowed and no parameter is a borrowed non-plain value. Ownership seeds the
capture slot from the same view row and checks closure returns against the
frame result. A copy of a Shared root is Global in both passes (MEM-11), and so
is a task or escaping closure's copied capture of a value whose only counted parts
are Shared, Mutex and Atomic handles (`copy_detaches`, CONC-2), as V's copy rule.
A deep copy (`unique x`) is Global in J apart from its arena or region placement
and crosses whenever its type does once unique; I keeps an already unique operand's
own relation. A taken noncopyable parameter keeps its row's crossing proof without
written `unique`, as a call receipt of its type does (MODE-4). J's
source rows are receiving contracts; where J specialised over a borrowed
capture whose current content ownership knows is local (a finalizer's value, a
record holding local content, consumed loop content), ownership publishes the closure's rows with a local result, and a
declared function slot cannot take it (MODE-1). Another closure captures it
as a slot local to the same environment; an inferred non-Plain Global result
makes that capturing closure local to the same environment too. A written
result header remains a contract. An inferred result promises no uniqueness:
J publishes a non-Plain inferred result aliased, since a capture read in the
body is a borrowed view whose share ends uniqueness (MODE-2), whatever the
captured binding's own row says.

A join of differently placed values has no placement and is as local as every
source, at every depth (PROV-4, `hirsel_types::placement_join`). An aggregate
holding such a part is local in J, and I's Forward rule bounds it by the frame;
`unique` and a deep copy of such content stay there (MEM-5).

Tests manufacture revision-1 tables and also run the real body checker over
the ownership, mode, destruction and closure source corpus. The conformance runner
checks only a reject's rule and line, so a site that only ever reports beside
another site (J beside I, or two I sites) could stop firing unseen:
`every_duplicated_checker_site_has_its_own_witness` finds such sites in the corpus
and requires each to report on a named witness file (`WITNESSED_SITES`).
`OwnershipFacts.unique` has one `UniqueDecision` per unary `unique` expression:
`AlreadyUnique` transfers/reuses the proven unique value; `DeepCopy` copies the
complete owned graph with Shared roots as boundaries. `Detach` is the third
explicit lowering action in the public protocol; this checker currently selects
`DeepCopy` when deep uniqueness is unproved, rather than proving only a root
count. For example `let a = unique borrowed_bytes` records
`UniqueDecision { expression: a_initializer, action: DeepCopy }`, while
`unique b"fresh"` records `AlreadyUnique`.
A deep copy of a function value or a type parameter's value keeps its source's
loans and relation, since a closure's borrowed captures are not copied (MODE-1);
V keeps the same sources.

`OwnershipFacts.guarded_patterns` has `GuardedPatternUse { pattern, test, commit }`
for every pattern node in a guarded arm. In
`match take r { Rec(items: xs) if valid(xs) => consume(take xs), ... }`, counted
payloads have `test: Borrow, commit: Take`. The test bindings are read-only;
a false guard releases them and continues to the next arm before any payload
commit. Plain payloads have `Copy` in both phases. The shared pattern/binding
UseKinds describe the commit, so the driver must pass the separate test uses to
lowering. Missing entries are a compiler contract failure, never permission for
lowering to choose an action.

Unreachable operations retain conservative `unique` and guarded-pattern rows,
and no reachable flow judgment. Lowering removes those operations before emitting
executable instructions; their rows do not establish reachable mode proofs.

Sites recorded in `allocating_mutations` require the OWN-7 runtime count test: a
single holder updates in place; sharing detaches into allocated storage.
Non-allocating mutation is statically proven. Published Shared and region storage
cannot be mutated.

The driver phase order is H → J → ownership → lowering. Production uses
`check_ownership_with_clauses(tree, item, signature, body, captures, clauses,
results, identity_joins, identity_fields, provider)`. `CaptureMode` is plain data with the closure expression, capture
ordinal, declaring `NameSite`, creation `UseKind`, and
`CaptureContext::{Scoped, OwningScoped, Escaping, Task}`. The type/layout remains
in shared `CheckedBody.closures`. For example a taken enclosing parameter has
`CaptureMode { expression: closure, ordinal: 0, owner: Item(definition_item),
usage: Take, context: Escaping }`; the driver converts J's capture output into
these rows without an ownership dependency on J. Absent shared capture rows do
not imply a creation move. Missing captures cannot establish complete checking;
the driver must provide J's full selection before publication. Task/scoped
capability capture restrictions keep their J-owned CLO-3/MODE-6 diagnostic;
operand availability remains OWN-2.

The ownership source corpus runs lex, parse, resolve, normalize, body check,
authority capture selection and ownership using **dev-dependencies only**.
It consumes `hirsel_driver::registry::BuiltinRegistry` signatures, including
implicit loop declarations, and the intrinsic roles in `hirsel_types::core_roles`. Tests convert the real
authority output's capture rows to `CaptureMode`; they never reconstruct free
uses, contexts or library callback types. Authority's enriched body feeds
ownership, while its independent diagnostics remain outside this I-only oracle.
Production ownership still has no authority or driver dependency. The independent
never-shrinking baseline is [passing.txt](../hirsel-ownership/src/tests/passing.txt), and
[pending.txt](../hirsel-ownership/src/tests/pending.txt) gives every remaining file and its
reason. `tools/bless.sh ownership-passing` verifies the existing baseline before
adding new passing files. Each operative ownership reject checks both rule and
source line. Accepted/run/fail files check for absence of ownership diagnostics;
this is a static oracle and does not claim runtime behavior. CLO-4's declared
capture-authority rejects belong to J; an escaping capture's locality and
mutability (CLO-4) and a once closure where Many is required (MODE-3) are I's
alone; Many is required by a binding, an argument, a part, and by the storage a
write replaces (a whole binding not written `once`, or a part's declared type). Scoped borrowing of noncopyable captures
uses the provisional option B reading of [spec #37](https://github.com/SamGalanakis/hirsel-lang/issues/37):
read-only resource/capability borrows permit many calls and never finalize the
capture. Taken or owned noncopyable captures and once function captures require
once; borrowing a once function does not permit its invocation.

Current ownership baseline: 1126 of 1358 files, with 1215 eligible for this pass.
The remaining 232 are listed with their phase/rule owners in pending.txt.

| Prefix | Verified | Eligible | Outside operative rules |
| --- | ---: | ---: | ---: |
| OWN | 222 | 260 | 1 |
| MODE | 696 | 730 | 114 |
| DROP | 63 | 76 | 5 |
| CLO | 145 | 149 | 23 |

## Authority, provenance and capture tables

Declared `uses` selections retain binder-exact parameter receipts in every callable
frame. Call restriction checking discharges a selected operand independently before
instantiating the callee's authority and joining callback effects; published authority
stays unknown. Forwarding never grants an unselected callback or ambient capability.

Uniqueness diagnostics trace a failed construction to its counted aliased operand, skipping plain fields. Channel payload diagnostics explain the unique payload/field spelling without changing the Sendable proof.

Fresh call receipts own their identities even when their types are Plain;
moving one ends dependent brands instead of detaching a copied alias. An error
mapper's parameter refinements and crossing/authority bounds are checked against
the owned error payload at the propagation site, independently of its success
payload and of the enclosing Result. Its uniqueness is I's: the failure edge
(`Op::MapError`) needs the Result currently unique for a `unique` mapper parameter
unless the payload is Plain or Shared. The same edge then returns the error
(`Op::Propagate`) under the checks an explicit `return Error(e)` gets: a nonplain
error read from a borrowed Result keeps its bound, unless a mapper function
returning a global result replaced it.

I alone judges current uniqueness. J's binding rows keep what they received:
no copy, related call or closure call weakens them, since only flow knows when
the holder of a share dies. J checks structure: a sendable bound, channel or
mutex payload and a taken task capture are judged as if the argument's root
were unique (`sendable_when_unique`), and nested rows are compared as written.
I checks every `unique` requirement, a sendable root and a taken task capture
(CLO-3) with flow evidence, and a core operation's own rule names its failed
argument requirement (`intrinsic_owner`): MEM-11 for `shared.freeze`, CONC-5
for `mutex.create`, CONC-6 for `channel.send`. A construction (record, variant,
tuple, sequence, box) is sendable when every part currently is
(`parts_sendable`), as V proves it field by field, so a part unique only by flow
evidence keeps its proof beside a handle part that leaves the aggregate aliased.

`hirsel_authority::check_file_with_clauses(file, item, signature, body, provider,
intrinsics, loops, clauses)` consumes the checked body and returns `AuthorityOutput`.
Argument labels are their `Symbol` spellings. Tests
supply the same syntax and producer tables as production callers.
The caller must reject diagnostics or `failure` before publishing the body.

The output owns these lowering/checking facts:

- `body.expressions`, `.bindings`, `.patterns`: immutable replacement type rows
  with call-origin substitution, branch brand unions and inferred closure modes.
  If/Match joins merge expiry before publishing replacement rows and include
  only paths that continue; mutation on a returning path cannot expire a value
  used after the join.
- `body.closures`: captures in first free-use order, with the capture type and
  **creation** UseKind. This is independent of reads inside the closure.
  A contextual `uses {..}` restriction is checked against the captures and body
  and retained on the published function type.
  Written function binding types supply that limit too; annotation refinement
  reads their resolver-owned capability paths before checking the literal.
  Untyped function bindings supply scoped capture context without a callable
  promise: the literal retains its own `uses` restriction and infers result
  provenance from its body.
- `captures`: declaring NameSite plus ResolvedName (nested ParamId(0) ordinals
  differ), first free-use span, creation use and type, authority union, and
  Scoped/OwningScoped/Escaping/Task context. A local once scope body can take
  captures without becoming global. Ordinary scoped captures borrow; escaping
  and task captures copy unless a free `take` selects ownership. Nested free
  uses participate in the outer capture selection.
- Brands and held authority remain on the enriched body type rows. Evaluated
  authority is private traversal state; a union/erased brand is diagnosed at its
  first branded operation.
- `capture_modes()`: `(closure ExprId, capture ordinal, declaring NameSite)`
  rows matching `body.closures` capture order. Lowering pairs each shared
  `Capture.binding` with this owner; parameter ordinals alone are ambiguous.
- `result_types`: `(NameSite, TypeId)` for every checked function frame, sorted
  by owner. Item functions use Item, named local functions use Statement, and
  closures use Expression. Each TypeId belongs to `body.types` and retains the
  formal result promise in the body-origin namespace; inferred closure results
  match the enriched ClosureInfo. Lowering must use this table for frame result
  types, rather than substituting the actual tail expression's stronger modes.
  ParameterRef::Body carries the declaring scope directly. Written provenance
  annotations refine the matching nested parameter bindings; calls substitute
  their own scope and retain outer references. H binds lexical parameter/result
  contexts before J; shared transport reaches outer references inside callbacks.
- `call_allocations`: invocation allocation promises joined with borrowed scoped
  callbacks (CLO-2); NoAlloc requires every joined promise to be NoAlloc. Task P
  and the driver consume these facts for allocation acceptance. Function branch
  joins also retain the weakest allocation promise. Every call that is not a
  constructor has a row, a call through a field included.
- `reads`: all resolver reads plus every successful or failed signature attempt.
  `dependencies` retain successful fingerprints. Provider operational failures
  stay separate from deterministic language rejection; no callee body is read.

`Intrinsics` maps canonical DefPaths to `Intrinsic`. The registry supplies roles;
user definitions with the same spelling acquire no special behavior. The registry
returns typed roles directly. Tasks/Scope capability identities are
shared NominalRoles metadata. Production identities are `core@1/...`; fixtures
supply these explicitly. Missing builtin capability roles conservatively yield
Unknown authority.

Erased `Iter` authority is conservatively Unknown: revision 1 has sendability
but no state authority field. To refine it, a producer would have to supply the
factory result ExprId, authority of each retained state/callback, and their
canonical join, then preserve that summary through bindings, projections,
branches and exported erased values. This is not inferable from `Iter[T]`'s
output element T. No new shared field is added. The relevant corpus is
`CLO/CLO-4-std-iter-run.hir`,
`MODE/MODE-4-iterator-state-bad.hir`,
`MODE/MODE-4-iterator-state-ok.hir`, and, for the ownership/state interface,
`OWN/OWN-5-iterator-copy-bad.hir`, `OWN/OWN-5-iterator-copy-ok.hir`.
J computes iterator factory sendability from every owned state/callback input's
receiving row. I derives the result's crossing mode again from each captured
argument's current value, as the validator does from the operand row I publishes,
so a let relation's frame bound on a taken binding does not make an iterator built
from global content non-Sendable (#88); I retains that construction-site evidence
through bindings. J's MODE-4 judgments keep the receiving rows. The
CLO case still needs a declared state/callback authority summary. The OWN cases are I's
noncopyability judgment, not J's authority oracle.

For example, `fn outer(outer_p: Dir) { fn inner(inner_p: Dir) { ... } }` has
`ResolvedName::Parameter(0)` in both functions. The reverse rows are
`(0, Item(outer), 0)` and `(1, Statement(inner), 0)`. `of outer_p` in the inner
body uses body origin Parameter(0); `of inner_p` uses Parameter(1). Capture
identities still use `(Item(outer), Parameter(0))`, so capture slots and origin
roots have explicit, separate interpretations. Closure literal owners use
Expression instead of Statement.

Driver order is body check, authority/capture check, ownership with these capture
rows, then lowering. Merge J and I diagnostics and select the first by docs/spec/diagnostics.md.
Task-handle obligations survive branch joins and are checked on each reachable
exit; cancel does not consume a handle, while await and detach do. Runtime task
execution, cancellation and joining are M2.

Tests begin with `hirsel-authority/src/test_support.rs` manufactured CheckedBody fixtures.
The real corpus in `hirsel-authority/src/corpus` runs lexer, parser, resolver, signature
normalization and H body checking as dev-dependencies, followed by J. Its
signature-only library fixture contains the core library declarations and nominal
roles. `passing.txt` is a never-shrinking list; `pending.txt` gives an owner and
observed failure for every remaining program; `annotations.txt` records spec
issue annotations (currently zero). Run `tools/bless.sh authority-corpus` to update the lists after the existing baseline has passed. Run/fail programs
assert static silence here; execution remains M2. Counts include selected MODE
4/5/6 and OWN 6 programs, and all PROV/CAP/CLO/CONC programs:

| Prefix | Passing | Selected | Pending |
|---|---:|---:|---:|
| PROV | 376 | 416 | 40 |
| CAP | 105 | 167 | 62 |
| CLO | 129 | 172 | 43 |
| MODE | 264 | 303 | 39 |
| OWN | 5 | 5 | 0 |
| CONC | 30 | 49 | 19 |
| Total | 909 | 1112 | 203 |

The 203 remaining outcomes are listed with their owners and observed failures in
`pending.txt`; the exact paths and diagnostics are checked in.
There are no unclassified J mismatches in this snapshot.

Projection regressions preserve H's structural type, complete scalar modes and
cleanup evidence for dotted paths and contextual fields. J refines only projection
provenance metadata. Constructor
fields select Escaping capture context using declared field types and source
labels. A real body/J/I regression checks `MODE-2-plain-part-preserves` and both
`CLO-4-stored-borrowed-capture` programs against their rule and line headers.

Ordinary call results specialize generic type contracts before applying allocator,
origin, lifetime, authority and erased-capture evidence from the actual arguments.
A concrete callable's access modes therefore cannot overwrite the arena placement
required by `copy_to`.

`hirsel_authority::check_file_with_clauses` receives H-authored implicit open/next
calls as shared `CursorPlan` rows forwarded by the driver. One refinement sequence
substitutes library parameter origins before caller generic graphs, derives both
call results, projects the item and updates the next instantiation. J supplies
evaluated authority while binding the loop pattern; only the source identity is published.
Borrowed views retain their original or union source brands. J retains each selected
source identity; after I refines current source content, `refine_cursor_plan` uses
the same sequence with structural authority from the current value rows.
The driver publishes that complete plan before K; lowering chooses no provenance
judgment.

A structurally proven plain expression is global and sendable (MODE-1/4),
including instantiated records, tuples, Option and Result. Conservative generic
root summaries cannot suppress these construction-site proofs. Counted payloads
prevent the plain proof and retain their declared sendability requirements.

Generic graph substitution takes the declared `NominalRoles` and normalizes
`FunctionType.error` from its substituted result, including nested callbacks.
The shared `result_error` helper recognizes Result by nominal identity. Pure
imports preserve existing channels; body checking needs no post-substitution repair.

Ownership publishes `hirsel_types::UniqueDecision` and `GuardedPatternUse`;
authority and ownership use the same shared `CaptureContext` and `CaptureMode`.
Phase-private copies and driver enum conversions have been removed.

Generic bounds inspect complete actual occurrences, including callback result
records in `ClosureInfo.result_evidence`. Contextual callable results remain the
checked promises; body inference does not strengthen those exported value types.

Plain, Copyable and Sendable proofs use hirsel-types with declared signature
lookups. Ownership no longer merges every actual for a nominal parameter into
one approximation. After flow refines locality/uniqueness, all published checked
roots receive shared derived summaries; provider reads remain recorded even on
failure. Function/Iter capture summaries retain J's explicit capture evidence.

Ownership flow state is one private loan set per value (`hirsel-ownership/src/flow/loans.rs`,
docs/spec/ownership.md, places and loans): a kind (identity, bound, read) on a place (a binding, a
lexical scope, the value's own environment or a placement a join lost), each with
its issue site. A value holds its alias, sources, captures and environment loans
itself, and its content holds read loans on bindings and scope loans by part. A
slot keeps one relation fixed at its declaration.

Ownership tracks local value sources separately from access loans. Copies of
counted local parts retain their source dependence. At a local(x) return, every
root must resolve to a declared source (or independent global storage); unresolved
cycles do not prove a source. This emits MODE-1 before later required mode errors.

Tasks/Scope capability identities come from shared NominalRoles, not J intrinsic
entries. Handle authority uses builtin_held_authority plus retained payloads;
phantom type arguments do not contribute held values. Operation capability
requirements remain separate and explicit. Erased Iter capture authority uses the shared capture contract.

Erased Iter types retain `Type.erased_captures`. Declared `FromArguments` uses
scoped `RelationPlace { parameter, fields }` selectors; shared instantiation derives known authority and mode
sendability from selected actuals. None remains unknown. Capture evidence survives
generic/scoped graph transport and v5 fingerprints; it is also valid on function values under revision 6.
The validator derives call results independently from their declaration.

The ambient heap is a typed Arena refinement. Bindings preserve its allocator target; authority is inert and lifetime is non-scoped. Calls use shared allocator-target substitution instead of recognizing a Heap AST argument.

`AuthorityOutput::capture_modes` publishes shared CaptureMode rows; ordinal width and declaring scope are established in J.

Call origin evidence now includes actual identity, retained brand and locality.
The scalar-position cursor declaration selects its PositionView argument's retained
brand through ArgumentBrand; J instantiates it once through the shared scoped
operation instead of replaying a separate identity map for every brand alternative.
Cursor result rows use shared normalization for Plain locality and sendability.

Sibling substitution uses shared instantiate_sibling_origins. The private recursive
rewriter and its missing-field brand/placement erasure have been removed. Unresolved
selectors remain explicit until the enclosing record supplies an identity.

Explicit take arguments expire the identity of the evaluated argument, the same
identity used to instantiate a dependent result. Detaching a copied local may
change that identity from the earlier operand read; consumed-result checks use
the argument snapshot so a returned brand cannot survive the take (PROV-1).

Identity traversal preserves operands only for take/copy/mut access operators.
Computed unary expressions have their own expression identity, which K introduces
explicitly and V checks through ordinary storage flow.

A nested ownership frame starts with its selected capture type's modes. Scoped
borrow permission is tracked independently; it does not relocalize a global
Shared root. A source already declared Local retains that restriction.

Cursor generic arguments describe stored items. Yielded item and destructuring
rows separately apply the shared projection rule to the borrowed next result,
so counted Map/Set payloads retain the source lifetime without changing generics.
Unmarked sequence iteration borrows its element independently of the source
expression's evaluation use. J projects the item from the checked sequence owner;
I refreshes inherited item and pattern locality from the physical source storage
after ownership refines it, retaining declared part contracts and Plain fields,
and uses the written marker to choose Borrow or Mut.
An owned Iter still yields owned items. Copying a source field never makes an
indexed element an independent owner.

Projected fields and destructured child binders retain their declared multiplicity.
A Once field cannot inherit Many from its aggregate: invoking it through a
projection is an OWN-8 partial move. Destructuring the whole owned aggregate
transfers the callable, and a second invocation then reports MODE-3.
Constructor and tuple roots keep their own multiplicity; a Once payload makes
the aggregate noncopyable through its field graph, without assigning the payload's
invocation count to the container. Generic relays therefore preserve both layers.

Dependent getter results instantiate both locality and placement from selected
argument types. Scoped placement makes the result Local even when construction
owned a Global value, and the ownership pass retains loans on every selected
source. Mixed scoped placements remain unknown rather than choosing one arena.

The authority walk feeds loop-header brand and identity judgments through the
existing transfer rules until their semantic state agrees. Operand identities
are refreshed at each evaluation; explicit breaks do not feed the header.
Fresh handles made inside an iteration retain that iteration's owner identity.

Nested generic callables establish fresh bounds; nongeneric callables inherit.
The authority frame and each ownership graph retain this context. Transported
outer types keep their declaring bounds through captures, aliases and value
markers. Property proofs for aggregates intersect evidence for occurrences from
separate binders that reuse a type-parameter ordinal.

Aggregate field contracts inherit explicit owner placement before constructor
argument checking or projection refinement. Plain fields and explicitly placed
fields preserve their declarations; borrowed reads apply locality afterward.
An element read takes its sequence's placement as a field read takes its
record's (PROV-4, OWN-8): an element of a list `in a` lives in `a`.

Call construction contexts instantiate parameter origins, allocator targets and
known generic arguments before placing fresh operands. Explicit operand placement
is preserved. Variant fields use the same comparison as record fields, and
constructor operand labels do not classify bare local reads as projections.

J uses the shared `structural_flat` root proof with its declared generic scope and
provider lookup. It keeps held authority and release obligations independent.

An immutable binding row is evidence of its single held value, including inferred uniqueness. A mutable local binding row is its lifetime receiving storage contract: explicit annotations retain their promises; an inferred root defaults to aliased uniqueness. Which promises are explicit comes from H's `PatternDecision.annotation` on the let's root pattern: J fixes written locality on the received row from it, and I's annotation requirement and mutable root storage contract apply its written axes. A `mut` binding written `@once` holds a once function whatever it received or is later assigned (MODE-3), so a call takes it and a second call needs a refill. Stronger current content evidence stays in ownership flow, separately from that row. Pattern bindings follow their own mutability. Replacement content rows derive from the receiving binding, rather than a stronger read occurrence.

Source storage paths come exclusively from `CheckedBody.places`. Ownership maps
scoped roots onto binding slots and treats every index of a sequence as overlapping.
Different Packed variants overlap; separate fields of the same variant remain
disjoint. J interprets checked field ordinals for flow-sensitive provenance without
reconstructing projections from spelling. Whole-binding replacement content uses
the binding's receiving storage contract; projected replacement content uses the
checked projected type. An assignment's right-hand side is an owned receiver like a
return or aggregate field: a borrowed or projected copyable value is shared into the
storage, and a borrowed noncopyable one is OWN-1 (a projected one OWN-8), whatever
the destination (issue #73). Only `let` binds a borrowed name as a view (LANG-15).

Escaping/task environments cannot retain a non-Plain row placed in an Arena or
Region at any depth of a captured value (tuple, record, variant, box and type
argument parts; callable headers are not entered); PROV-4 cites the closure
literal. Scoped environments retain the
parent's placement origin for their borrowed capture.

Borrowed iteration attributes non-Plain element views to the checked container
root, retaining an existing source dependency before falling back to its binder.
MODE-1 local-result source checks therefore name the original parameter, including
field sources; Plain elements still copy independently.

A plain `mut` argument ends its root's uniqueness, whole or projected, since the
callee may install a share anywhere in it (MODE-2). A `mut` argument that is a part of
a currently unique root is itself unique, so it meets a `mut unique` parameter and
keeps the root unique. A directly called declaration with `@preserves_unique(p)`
(`callee_preserves_unique` checks the callee resolves to a definition) hands a selected
`mut` argument back unique, or returns a unique result for a selected `take`
parameter, when the selected and every non-plain taken argument is unique at the call
(`unique_arguments`), and, for a bodyless core@1 callee (`callee_definition`), every other
`mut` argument is unique too and every borrowed argument is Plain or Shared, since a ledger operation may store a share of it, unless the
promise lists it `unretained` (`callee_unretained`: `sorted_map.insert`'s `by`); that
result uniqueness is flow evidence only, so the published call row stays the
declared one, as V derives it. A `mut x: unique T` parameter must be unique at every
return and evaluation failure site (`mutable_crossing_exit`); a related call result's
counted shares only mark it `borrow_only`. A failure site accepts that, and so does a
return whose value reaches none of it (`reached_bindings`). A temporary released after
its last use stays `parked` on the storage until its statement's `StatementEnd`. An early
exit (`break`, `continue`, `return`, `?`) passes the `StatementEnd` of every statement it
leaves, innermost first, up to the loop it stays in (`exit_statements`);
`restore_borrowed` makes the storage unique again, at each statement end and kill, once
no live binding, evaluated temporary or parked temporary reaches it. A call whose
type is Never has no successor node, so a match arm it ends adds nothing to the join, and a
bare nullary variant is fresh, as a literal is. Releasing a value with user-visible
destruction (`destroyed`: noncopyable or once) at a kill, at the end of a statement that
holds such a call temporary (`destroying`), or by a displacing write is a failure site
for the hand-back. A parametric call (`parametric_unique`, from
`hirsel_type_analysis::parametric::parametric_inputs`) returns a unique value when every argument that may
supply its result's type-parameter values is unique; a core callee counts as bodyless. A taken argument of
a call reaching it makes the share real (`share`). A `@preserves_unique`
body is checked a second time (`flow::check` with a proof parameter) with its
selected and non-plain taken parameters entering unique (`proof_entry`), other `mut`
parameters as declared; a selected `mut` parameter
must be unique at the same sites, a selected `take` parameter's result at every
return, and only the promise's own diagnostic is kept from that run.

Projected replacement preserves the root's uniqueness for Plain bits and Shared
boundaries. Storing aliased counted contents weakens that evidence, so a later
unique call is rejected under MODE-2 at the call. Whole-storage contracts remain
fixed; a fresh unique replacement may preserve the current root evidence.

Map iteration retains a whole-container loan until every loop exit, including an immediate break, independently of element liveness. Mutable map patterns retain H's readonly-key decisions; only the value rows become element places. Owning cursor provenance derives the item from the checked next result rather than assuming one cursor type argument.

Named local functions and literals use the same capture-creation transfer. The
nested frame reads the published capture modes, while its borrow permission
remains separate. Function binding modes are published on the callable row,
not on its body expression or its parameter rows. Computed projections apply
the declaration-owned field lifetime to the current root value; a prior Local
read refinement cannot relocalize a later Global call result. Inferred Local
parts (a mixed-placement join) keep their lifetime.
Branch continuation joins retain an alias identity only when every continuing path agrees. Identity changes and expiration include continuing paths only; a returning arm cannot change the surviving path's alias. Loop convergence includes both alias identities and identity-change sites.

Divergent aliases receive their binding's fresh identity at a continuing join. J publishes the control site, binding owner and origin in AuthorityOutput.identity_joins; agreeing aliases retain their original identity. Loop heads join entry and feedback identities with one finite introduction per binding/head. Old evidence can remain valid against an unchanged original owner, while fresh evidence formed against the joined binding names the new identity.

Identity introductions are restricted by I's initialized-binding proof at the same expression convergence or loop head. Returning paths do not enter that fixed point; bindings moved on any incoming path receive no introduction. K transports the remaining J/I rows, so naming a join never reads unavailable storage.

Contract clauses have distinct readonly parameter rows, including through aliases
and nested closures. Their ownership graphs do not consume the ordinary frame's
parameters. Clause parameter access violations use FACT-1 at the whole clause;
ordinary type, locality and copy constraints keep their normal diagnostics.

Projection lifetime plans cover both computed roots and binding roots. Reads use
the receiver's current lifetime only for parts declared Global; an inferred Local
part retains its lifetime after a fresh whole replacement. A field writes no
locality (LANG-7): it is as local as its owner and its current content.

Authority flow tracks copied field-prefix identities separately from whole-value
identity. Unchanged siblings keep their source identities through projected writes,
subrecord copies and captures. Control joins publish checked prefix types alongside
whole introductions; lowering transports these judgments through existing origin
bindings, and ownership readiness suppresses introductions on unavailable storage.

Write publication retains the final fixed-point receiving availability alongside
its content type. Some(true) is live, Some(false) is dead, and None is mixed.
Whole writes can initialize moved storage; projections and compound assignments
still require live storage. K consumes the judgment without reconstructing I flow.
Nested callable uses limits bound incoming captures. A supplied parameter or valid
call may derive a different capability inside the frame, including its projected
parts. J keeps capture restrictions and call provision distinct from ambient
callee requirements; known attenuation does not add types to published uses.

Whole assignment to a transferred owner ends its old brands and establishes the
receiving binding's identity. Later calls select that identity, matching K's fresh
storage introduction. Projected writes retain the enclosing namespace, and Slots
insertion/removal retain their container identity.
A constructor that moves an owner into a field holds that owner's identity
(PROV-2: Take transports evidence), and moving the value moves the hold. A written
take or write of the holder at or above that field ends the held identity, so a
value branded by the moved owner outside the record is PROV-1 at its next use,
as V judges it. Copied and Plain fields hold nothing.

Pattern rows retain the physical scrutinee type at every node. Receipt modes are
published only on BindingDecision.ty: destructuring an aliased whole weakens a
counted receiving value, while taking from a currently deep-unique whole gives
counted receipts unique evidence up to a Shared boundary. Declared-aliased
intermediate fields do not erase that stronger value evidence. An unshared
owned call receipt also retains its declared unique payload contracts.
The use judgment is shared by both rows. Unused and wildcard nodes still provide
the physical field type; their lack of reads cannot change projection validity.
Ownership binding slots use the shared `hirsel_types::rows::Rows` container.
A flow snapshot shares
unchanged branches, a write copies one eight-row leaf, and an idempotent join
visits differing rows only. Availability bits and complete Value judgments retain
the existing fixed-point rules. `work::read(Counter::CopiedSlotRows)` counts copied leaf
rows on this thread; a driver regression bounds let/closure snapshot work without
wall-clock thresholds. Private initial rows and expression temporaries remain
separate from the published mode decisions.

Fresh boxes, records, variants, tuples and sequences publish deep uniqueness before
generic and receiving checks when their actual operands are owned unique values,
Plain bits, independent flat copies or Shared boundaries. A unique borrowed or
projected counted source does not supply that evidence through an implicit share.
J types an inferred tuple or box part by its operand's row, which is evidence,
not a contract: I keeps each currently aliased part of a value in
`SharedParts`, with the operand that put it there. It is set at the
construction, follows the value through moves, projections, joins and part
writes, rejects a nested `unique` contract the row claims but the content
lacks (MODE-2, at that operand), and is published: the expression and binding
rows that hold such a part get it aliased (`shared_parts_row`).

Comptime wrappers retain their child's complete refined occurrence after J,
including substituted call origins and nested types. O uses that checked row as
the synthetic evaluation result contract before executing or materializing it.
Ownership frame inputs share three private binding views: inactive, owned outer
storage, and scoped borrowed outer storage. Each graph initializes only its own
binders and selected captures through ordered source indexes; all foreign bindings
retain their former live/borrow policy. Publishing a changed binding mode refreshes
that row in the primary views before the next graph. The old whole-definition
initialization loop and vector-to-row conversion are removed.
`work::read(Counter::OwnershipInitializedRows)` counts actual initialization inspections; a
closure-source gate bounds work independently of wall-clock time.

Retained Local-result permissions follow known aggregate content in private
ownership flow. Record/variant argument ordinals, tuple fields and destructuring
paths come from checked rows. Replacing a field removes only its installed
content dependencies; copied fields and unchanged siblings retain theirs. A
sequence element write retains the container's summarized dependencies. Whole-value
receiving/access bounds remain separate from per-part content bounds; the root
summary is computed on read, and projection selects only content bounds. Joins
union continuing alternatives, and recursive construction summarizes beyond the
maximum checked access/pattern depth so the flow lattice stays finite. This does
not strengthen receiving types, pattern physical types or uniqueness judgments.
Owned Local aggregate field receivers preserve the checked projected content
lifetime, just as whole receivers do. Caller-borrowed aggregates still use the
Global stored-content contract; their localized access type does not authorize
installing a new Local view. Uniqueness and physical pattern rows stay separate.
The private slot receiving lifetime is established at binding and survives fresh
Global replacement; current content may prove stronger without changing which
Local values that slot can subsequently receive.

Postcondition borrowed formals use the must-joined uniqueness of continuing
parent returns; preconditions retain entry evidence. Clause graphs run after
their producers. The internal clause header is rebuilt from those checked
formal rows, so a plain mut call cannot leave a creation-time Unique promise.
Physical source projections and received values have separate contracts.
`OwnershipFacts.physical_projections` publishes declaration-owned field modes
with OWN-8 locality from the physical owner, including selected capture storage.
Every `Field`/`Index` step below a read place publishes its row too (`xs[0]` in
`xs[0].n`), since lowering projects each step. A projection rooted at a block
takes its physical root from the block's innermost tail place (#50), the storage
lowering projects from. A computed root (a call result) is consumed by the
innermost step, which publishes its row while the root is live; an outer step
folds its own part over its live base step (`f()[0].name`).
The expression row retains the read/copy receipt; K transports that receipt when
materializing a value. Pattern fields similarly inherit their physical aggregate's
locality from declaration-owned part rows, before owner locality was inherited,
without taking the binding receipt's weaker uniqueness. Inferred Local parts
retain that contract; Plain fields remain Global. Borrowed lets
retain source storage, while matches stage their checked scrutinee.

Inferred tuple and Box part contracts describe the value received by construction,
separately from a projected field's physical storage contract. Reads of copyable
parts through computed borrowed roots copy those parts, preserving Local source
lifetimes; Flat copies own independent bytes and are unique, but stay in the
borrowed owner's locality exactly as a binding root's part copy does (OWN-1). Explicit payload annotations remain
requirements, and counted sharing cannot establish deep uniqueness.
Local prepared-source lifetimes are lexical CFG scopes, rather than a function-
return-only flag. Receiving bindings, writes, block values and constructors must
stay within every retained source scope. Aggregate content partitions carry those
bounds together with loans, so projecting a sibling can forget an unrelated
temporary. Whole Local binding replacement retains its declared receiving bound.

Constant-root projections derive physical field modes from the constant signature,
just as binding and computed roots derive their declared storage paths. Receiving
copies keep separate aliased occurrence modes; reading a declared unique field
cannot turn a shared constant receipt into an exclusive value.
Authority identities use primary snapshot rows keyed by exact scoped binding,
with root-grouped holder rows maintained by every overwrite and removal. Whole
and field-part identities retain independent holders. Scope exit checks only
holders of its computed root; branch joins compare changed snapshot rows and
keep the existing fresh-identity and diagnostic policy. The old flat identity
map and whole-holder searches are removed. Authority and ownership use the same
`hirsel_types::rows::Rows` container; `work::read(Counter::AuthorityHolderRows)` counts inspected
holders in the cold closure regression.

Authority invalidation indexes immutable type-row brands by exact root, advancing
only over newly appended rows. Candidate IDs keep arena order and retain the
same field-overlap check and most recent ending site. Mutable-parameter handback
inputs are collected once in original binding order, including duplicates; their
current types and already-reported state remain flow facts. The repeated whole
type/binding searches are removed. `work::read(Counter::AuthorityInvalidationRows)` bounds cold
closure candidate work without relying on elapsed time.
Projection receipts resolve dependent call locality after specializing their
receiver. Inferred tuple and Box children use that resolved locality, while
physical field contracts retain their declared modes and cleanup evidence.
J checks nested uniqueness against the shared non-writing view relation. A typed
binding's nested contracts remain its annotation after receiving stronger evidence;
whole replacement checks the binding's storage contract, rather than the old
value's stronger occurrence row. Current root evidence remains separate. Mut
hand-back and copied receptions preserve nested invariants.

Consuming Local cursor items carry a storage access scope separately from retained
view content. Acquiring an owned copy ends its cursor access dependency. A result
or borrowed capture retaining the item keeps the source bound, so a view cannot
escape the cursor through a longer-lived receiver. Content partitions and joins
transport both scopes. Registry promises decide which loop sources are admitted.

Authority binding-type snapshots use the same `hirsel_types::rows::Rows`
container as other flow facts. Joins enumerate the union of changed rows in binding-key order;
unchanged bindings keep their existing type ID. Type qualifiers and the existing
recursive join rules remain the authority for changed bindings.

Authority source places, calls, patterns, scoped bindings and parameter rows use
first-row indices into their existing arenas. Closure/capture construction facts
are primary keyed rows with an insertion-order index; replacing a fact keeps the
former remove-and-append traversal order. A mandatory current-type index follows
closure mutations, including receiving promises and inferred results. Publication
produces the same sorted shared vectors. `work::read(Counter::AuthoritySourceRows)` counts
construction, query and residual candidate work across these tables.
Projection physical rows retain the declaration-owned complete field or element
contract, including callable headers. J-resolved owner placement and brands are
transported separately; expression receiving modes never replace that contract.
A callable inside a received function value's header keeps its `uses` and
`@noalloc` promise in the direction it is read (`hirsel_types::Variance`,
CLO-2): a result may promise more than the receiving type, a non-`mut`
parameter may ask for less, a `mut` parameter is exact. A received value's
parts follow the same edges: tuple, record, variant and box parts and
sequence elements may promise more, while a type argument of a nominal type
or of a non-sequence builtin (`Option`, `Map`, `Shared`, `Iter`, ...) is
exact for a callable's promise, an erased state's captures and a cleanup
summary (`keeps_exact_promises`), since the declaration may use it in either
direction. v22 is silent there; this is the conservative reading (spec #92).
A header row's own uniqueness follows the same edges (MODE-2,
`hirsel_types::header_weakens_uniqueness`, shared with V): a `unique` result
meets an aliased one, an aliased non-`mut` parameter a `unique` one; rows below
it stay exact. Header diagnostics name both function types through
`hirsel_type_analysis::spelling::type_spelling` with modes shown.

`@noalloc` is a promise that is proven (MEM-5). A function value is received
by a `@noalloc` function type, at the root or as any value part, only when its
row promises it (MEM-5 at the reception). An inferred closure literal's row
carries the evidence its body proves, as it carries capture-derived `uses`
(#58): `noalloc_evidence` accepts a closed set of forms that cannot allocate
(no block built outside an arena, no `unique`, writes to Plain bindings only,
every call `@noalloc` after the CLO-2 join, no `for`, no contracts, every
owned value destroyed without allocating, nested closure literals only where
they cannot leave the frame). Anything else has no evidence; a written
`@noalloc` on the receiving type asks the allocation audit for the complete
proof instead. The row of a literal that initialises an unannotated binding
also carries the cleanup summary of the values its environment owns, and an
unannotated binding takes its value's summary.

A written function type in a binding annotation is the binding's contract at
every depth (#58): `annotation` restores `uses` (or its absence) and
`@noalloc` on each written function type, the initializer is kept against
that, and an annotated function binding's row states the written `uses` and
allocation, not what its initializer proves; a mutable one also the written
authority, while an immutable one keeps the authority its value fixed (CLO-1). A `mut` argument's
callable promises equal the parameter's exactly, root and nested
(`keeps_handback_promises`, CLO-2); the diagnostic quotes both types where
they have a short spelling (`function_spelling`) and suggests the binding
annotation or a narrower parameter. A dotted callee (`h.action()`) is called
by its own row, not its root binding's. CAP-1 provision compares what the
callee needs beyond its operands with the operands whose authority is known.

Ownership reads current expression, place, call and closure values through
private first-row position indices built once per checked body. Binding-site
groups select lexical declarations without whole-binding scans. All CFGs share
one immutable capture-metadata table indexed by closure and ordinal; graph
clones share it too. The source-shape regression counts actual source candidates
for successful ownership and allocation-audit publication.

Explicit mutable binding locality comes from its checked declaration, separately
from stronger initial content. Projected writes retain unknown old dependencies
conservatively and install incoming bounds at their field path. Local content
weakens the current deep-Global proof without changing receiving or physical
storage contracts; joins preserve that bound on every continuing alternative.

Lending an Iter or function binding mutably resets its current producer Sendable fact to
its fixed receiving contract: Local receiving storage permits a non-Sendable
replacement. A Global iteration staging slot keeps its declared crossing mode.

Shared property proofs propagate nonregular expansion as QueryError::Operational.
J's snapshot adapters retain that error in AuthorityOutput.failure; I propagates
it through the query result. Conservative intermediate facts cannot publish a
language rejection or an accepted body after proof tooling exhaustion.

J's physical layout uses shared `expand_representation` with its evaluated
argument authority and recorded signature reads. Resource and physical-layout
walks use the same instantiation-aware nominal expansion tracker as property proofs.
They traverse finite graphs without a 128-declaration cutoff; completed siblings
do not spend active growth budget.
Nonregular polymorphic growth propagates the declared operational failure.

Authority header and expression checks return verdicts independently of diagnostic
storage. Calls begin their prerequisite proof after callable lookup; constructors
check fields only after valid arguments. Sibling-brand checks select PROV-5 before
emission, and placement reports use one emission-site rule/span set for deduplication.
Declared-row joins use shared `Modes::join`, retaining J's widening of conditional
localities and the body checker's seed modes for empty joins.

I consumes J's identity introductions and retains only those whose binding is
initialized on every incoming path. The shared `hirsel_types::OwnershipFacts`
transports those joins, field projections and ownership decisions directly to K;
the driver does not filter live slots or copy a second ownership record.
Field identity projections are bounded by the final receiving binding's relation,
through every path component, after ownership refinement. Their sibling brands
and nested callable contracts remain J's judgments.

`OwnershipFacts.plain_types` retains I's Plain classification for the final checked
store. Mode-only flow refinements reuse their source classifications; appended
normalization and replacement rows use the same declaration-based proof. K reads
this set for cleanup, without a second driver-wide property pass.

Written sendability is a receiving contract for every type, at every part.
Receipt checks current content before permission defaults, retaining checked
occurrence evidence through moves and borrowed views. Counted sharing and writes
update that evidence; a declaration never restores it. Mutable hand-back compares
fixed receiving contracts independently of stronger current data evidence.
Function and Iter environment evidence remains independent of a view's lifetime;
real content and placement bounds still prohibit crossing. Qualified callable
parts supply literal context, while captures establish their evidence. Conditional
promises retain their exact binder scope and accept stronger proved evidence.

Current owned-part crossing proofs track shares, partial replacement and joins
separately from physical receiving types. Qualified call inputs, results and
mutable exits check those proofs. Generic actual admissibility uses the callee's
signature-derived copying permission, including Data/Key/Flat/Plain implications
and selected borrowed-result reads; changing a callee body cannot strengthen it.
Expression rows publish current evidence without inheriting their source's root
requirement. Postcondition inputs carry continuing-exit evidence. Copying a mutable
iteration element or replacing it with unproved content invalidates dependent
proofs on its collection as well as its binding; replacements keep their element
provenance for subsequent reads and writes; opaque function transport retains captured-input obligations.

Variant argument brands follow the selected supplied payloads. Payload-free variants contribute no brand constraint at construction or a join; different present source brands remain a union. Declared uses selection proves exact callback parameters independently of unknown ambient summaries.

Revision 6 publishes nominal borrowed-loop associations and scoped callable state
promises. Core Unicode views use their nominal declarations rather than collection
selector labels. `hirsel-ownership/src/flow/state.rs` proves every promise of the graph's own
callable (item, closure literal or local function) on its binder-exact, field-
sensitive selection: readonly use, Plain or stable copies, invocation of many-use
callables, closed-content bookkeeping and `mut` calls whose declaration preserves a
covering region retain the incoming envelope; replacement, unpromised mutation,
sharing and mutable capture export reject on every path.
Whole-input creation promises additionally follow actual retained operands through
moves, bindings, joins and plain nominal record transport. Unknown producers,
unproved copies, resource construction and unpromised mutable effects cannot
establish an incoming envelope. Unselected inputs need independent inert, Global,
Many, Sendable and noalloc-drop evidence. A mutable factory callback relays the
exact incoming state/callback pair, promises to preserve its whole state slot (a
fresh closure receives that promise from its creation context and proves it), or
has a closed slot with a Sendable receipt; an inert state with a Sendable receipt
needs nothing more. A closure literal whose callable type selects `@captures` is a
creation owner proved like a header. Field-selected and callable-result creation keep
their guards.

Qualified mutable hand-back is checked on normal, error and fail-stop exits.
Source replays call/constructor/capture and checked-expression failure boundaries
after operand sharing; releasing local sharers never restores crossing evidence.
Normal-return summaries remain separate from this fixed caller-owned obligation.

Known closure literals and immutable bindings retain sharing sources from
Global captures as well as arena-only captures; unplaced Global data is its
own share. Local and taken arena storage keep their frame-bound checks.

Parameter field-source replay uses a private trie of the frame result's declared
relation prefixes. Matching and divergent paths are absorbing states; only strict
prefixes remain distinct. Projection and union commute with the original prefix
predicate, so recursive list traversal converges without an iteration limit or a
weaker result-source rule. No expanded parameter path is retained in flow values.
The corrected shared-list benchmark is a LANG-2 reject at its untyped indexed
bytes literal; `hirsel-pipeline-tests/tests/front/modes_flow_solver.rs` checks that exact specimen and its accepted
explicitly typed twin through the complete source and independent IR pipeline.

Forward ownership propagation uses a reverse-postorder priority worklist. A
solve-local exact ordered slot-join cache preserves the existing slot operator,
shares repeated inactive subtrees and reports changes during the merge instead
of comparing whole snapshots again. Access queries select binding holders from
backward demand in their former binding order. Parameter-prefix construction
uses the existing scoped binding index. `hirsel-pipeline-tests/tests/front/modes_flow_scaling.rs` checks 32/128
persistent slots and deep branch-local owners through lowering and independent
V publication, bounding transfers, value joins and actual copied rows by eight
times the smaller case plus startup slack; no timing or iteration cap gates it.

CAP-5 whole seals always charge their nominal identity. Owner finalizers use the
private implementation ceiling. Dotted and postfix projections and immutable
receipts retain value derivation inside their creating frame; capturing an
extracted grant charges that grant's own type. Scoped authority is specialized
through generic/erased holders before escape checks, including empty local seals.

Serializable instantiation evidence uses the shared checked-schema proof; it
supplies Inert generic authority through the common bound implication helper,
while copying, lifetime and current Sendable proofs remain independent.

The registry-backed source harnesses import signature normalization from
hirsel-signature and body checking from hirsel-check directly. Both dependencies
are test-only; production modes continue to consume shared phase data.
The ownership corpus classifies a rejection as J-owned only when J actually
reports the expected rule at the expected source line. Its existing passing
ratchet still requires I to retain every previously proved ownership judgment.

Loop pattern bindings name the yielded value, separately from the collection or
cursor source. Each iteration introduces a fresh binding identity, including
borrowed indexed items. The projected item's existing brand, locality and arena
placement remain unchanged. This lets `string.find(item, ...)` return positions
of that exact item without claiming the collection itself is a string owner.
`PROV-1-for-item-find-run` and its rejected twin retain the same-owner requirement.

Equality context for a payload-free variant is refreshed after both operands'
call results have their caller-side substitutions. Its type arguments retain
that complete context, while the empty construction retains no source lifetime.
`LANG-2-option-comparison-context-run` covers bare/called `None`, operand order,
block wrappers, generic results and map lookup; the Slots control checks the
original static case without depending on unfinished Slots execution.
Mutable yielded bindings separately retain their collection's write path. Item
writes and `mut` calls renew that collection through ordinary alias detachment,
while the item's fresh value identity still types position-returning calls.
The existing mutable-container PROV controls and mutable-item call twins cover
both assignment and call mutation without weakening their expected diagnostics.

Diagnostic origin names recover a fresh initializer's declaring let when it has
one. Descriptive owner names keep their prose classification when embedded in
`of`/`in` qualifiers, so internal rendering markers never become source spelling.

Ownership retains an unshared owned call-receipt root separately from deep
uniqueness. A consuming Result propagation or pattern can move its declared
unique payload; copying or sharing the receipt invalidates that evidence.
An unqualified counted payload starts a separate root without that proof.
Propagation owns its Result independently of the success value's consumer.
Uniqueness diagnostic causes follow counted aggregate inputs separately from
locality causes, so a plain sibling cannot become the cited counted alias.

Explicit take capture classification follows transparent block tails (LANG-11),
so `take { x }` captures the same owner as `take x`, including resource and task
captures. Statements inside the block retain their ordinary evaluation order.

A nominal `@format(f)` association implies `@preserves_unique` for f's second
parameter (SEM-3). Ownership reconstructs that promise from the receiver's
declaration for direct calls and re-proves f's body, including unused functions.
Failed proofs report MODE-2 at f with a note naming the association; an explicit
promise is redundant and runs the same proof once.

Ownership CFG environments retain all active lexical loop targets and labelled blocks. Labelled edges end statements and scoped bindings up to their selected loop. Authority task obligations follow that same target, and header convergence includes labelled continues from nested loops. A labelled block is an `if`-like join (LANG-11): each break operand is evaluated in the block's context, forwarded to the block, then ends the scopes and statements it leaves before the block's `IdentityReady` join; `Graph.block_breaks` adds break operands to every transparent value-position scan, while transparent tail chains stop at a labelled block. The authority walk joins each break's path state with the fallthrough as a match joins its arms.

Contextually instantiated named function references rebuild their declared callable
with actual generic authority before receipt. J proves sendable/inert bounds at
creation and substitutes authority/uses throughout parameters and results. Indirect
calls retain the declared modes, allocation and relations; preserves_unique stays
direct-call only.

Diagnostic repairs with a known mechanical edit carry `Diagnostic.fix`: last-use
ownership inserts `take`, a counted part leaving its owner's block inserts `copy`,
and a redundant last-use copy is replaced by its local binding. Insertions retain
the original expression text, including qualification, labels and comments.

Lexical generic function values use the same contextual instantiation and capture
creation rules as closure literals. J reconstructs the declaration from its lexical
binding, checks crossing/authority bounds, and publishes a capture table and formal
forwarding result. I checks its construction through the existing Capture operation;
the lexical generic body's own frame remains checked at its declaration.


Private helpers publish body-checked `UniqueSummary` evidence beside their declared
signature. The engine settles each same-module call-graph SCC before freezing front
headers. `FunctionType` stays declared, so indirect calls and generic inference cannot
consume these bits. Ownership reuses the preservation proof and independently tracks
result root and direct Result/Option/tuple payload evidence. V re-proves every published
bit on the callee, and consumes peer bits only at direct calls in the same module.
