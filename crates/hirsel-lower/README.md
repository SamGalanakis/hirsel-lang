# hirsel-lower

Lower mode-checked source decisions to ordered TIR with explicit storage,
loans, cleanup and failure paths. Missing or inconsistent facts fail closed.

Allowed production dependencies: hirsel-span, hirsel-intern, hirsel-syntax,
hirsel-types, hirsel-diag, hirsel-tir and shared hirsel-check-protocol plans.
No signature-provider, modes or driver implementation is invoked here.
The driver enforces successful checking and mode analysis before calling lower.
The interpreter and independent IR validator are dev-dependencies only; tests
supply manufactured phase decisions and declaration signatures.

`lower(Input)` consumes the retained `SyntaxTree`, a **mode-checked**
`CheckedBody`, producer `Plans`, the definition/owning `NameSite`, signature
hash, root expression, J formal frame result rows, and unit/bool TypeIds in
`CheckedBody.types`.
The driver must enforce successful H/I/J before calling it. It returns an owned
TIR `Definition`, or a `LowerError` for missing/inconsistent compiler facts;
these errors have source spans and are not language diagnostics.

Resolver bindings map to independent TIR slots; nested parameter owners are
preserved. Source uses and capture-at-creation uses come from the checked tables.
Synthetic Take operands only relocate already-owned compiler temporaries.
Instructions preserve written evaluation order, and call/constructor ordinals
map the evaluated inputs to declaration order. `CheckedBody.calls` supplies exactly one checked mapping for every call, including
positional named calls. Each source-order argument carries its formal ordinal and
specialized parameter requirement; recovery is explicit.
`Plans.propagation` carries nominal Result ordinals/identity, never guessed from
spelling. Shapes and materialized comptime constants use explicit producer plans.
Local functions use a producer-provided closure expression/capture identity.
Error mapper plans retain their declared passing mode. Lowering moves the error
payload into owned temporary storage, then lends it under a read loan for a
borrow mapper or transfers it for a take mapper; failure cleanup retains the
borrowed owner. Capability-typed constructor fields receive ordinary type checks;
the missing-capability call exemption applies only to function calls.

CFG construction carries live-root sets, joining with conditional drops.
Cleanup ends loans, then releases each exited scope innermost first: its
temporaries in reverse evaluation order, then its named owners in reverse
declaration order (DROP-2). A loan held by a value with user-visible destruction
ends after that value's drop. A borrowed argument temporary that a `let`
binding's value keeps is a named local declared just before that binding.
Scope exits extract their result first. Replacements evaluate RHS, atomically exchange the live place into an owned temporary with Replace, then drop the temporary. Caller storage stays live if finalization fails. Call preparation
materializes each argument before the next; failure edges retain earlier owned
temporaries, while entered calls exclude their taken inputs. Parameters/captures
borrowed from their caller do not become destruction obligations.
An indirect call borrows a stable source callable, including global projected
implicit shares. Scoped projected shares keep their existing content-proof path. A bounded
source-effect check refuses moves/writes to its root, retained
exclusive aliases, and unsupported hidden effects during later arguments. An
unproved source keeps the owned snapshot before argument evaluation, preserving
the selected environment through replacement. Ordinary read loans and independent
V validation govern both paths. Borrowed results retain capture dependencies;
owned erased environments additionally retain their source access loan through
the result's last use. Projected selections are checked before later arguments
under a disposable read view, with no owned share. Normal and failure cleanup
end loans before releasing their source owners.

`print::definition` emits a deterministic arena listing for snapshots.
Source fixtures that run H/I/J and K through the driver live in
`hirsel-pipeline-tests` (`check_*` modules). Isolated lowering tests here supply
explicit phase facts and declaration signatures, then independently validate TIR.
The interpreter is a **dev-dependency**; production consumes shared phase data only. Unsupported producer facts
fail closed (missing ownership sidecars, cursor declarations, contract
before/result and unmaterialized comptime). Logical frame bytes remain
zero until #16 supplies a charge model; this field is not an allocation estimate.


`Input.ownership` consumes shared mode rows, with no hirsel-modes dependency:
`OwnershipFacts.unique` contains `UniqueDecision` rows with an ExprId and
`UniqueAction::{AlreadyUnique,DeepCopy,Detach}`.
Reuse requires Take; deep copy/detach require Copy into the complete destination
type. Detach conservatively copies; optimization may reuse only with equivalent
observations. `guarded_patterns` contains shared `GuardedPatternUse` rows
with separate non-consuming test and commit uses. Guard bindings are views;
loans end before the true edge commits a move. `capture_modes` maps closure
ExprId/capture ordinal to J's declaring NameSite where scoped identities would
be ambiguous. The driver converts J's CapturedName.owner without guessing.
`Plans.iterations` selects H-checked integer `RangePlan`s. Inclusive loops stop
before incrementing the final element, including MAX..=MAX: the step continues
only while `counter < end`, which the header's `counter <= end` makes equivalent
to `counter != end` and which proves the header's next test, so optimized code
tests the bound once per item. Borrow/mut call
arguments retain the original caller place, so mut operation updates reach it.

`Plans::from_body(tree, mode_checked, &hirsel_check_protocol::body::Plans, root_expression)` adapts H's landed producer output. It preserves call evaluation
order, multiple path-field ordinals, registry Result identity (including function
and constructor error mapping), closure/local-function result owners, checked
shape arguments and integer ranges. Propagation tags and target result types come
directly from H's TryPlan. Local functions use their syntax body as J's capture
identity, without a separate statement-to-closure transport. Indexed
sequence protocols use H's checked length type, length definition or materialized Inline size and
explicit index projections. Definition-backed Iter advancement uses checked
instantiations and Option tags, with receiver mutation reaching its owner. Intrinsic
cursor protocols consume shared `CursorPlan` rows from `Plans.cursors`: open/next
identities, source use and complete Option layout were selected by H and refined
by J. K does not receive a second registry mapping or partial result-type table.
Both call instantiations remain recorded on the source ExprId. Missing/duplicate
plans, invalid result types, missing actuals or local-function captures fail closed.
Borrowed protocol cursors retire silently before their last-use loan ends on
the normal path; failure cleanup keeps the loan through cursor destruction.
Borrowed pattern
bindings retain scoped source views, so successive cursor iterations do not
reinitialize a live borrowed alias. Tests execute all registered operations through
the real interpreter library and cover continue, break, return and failure.

`Plans.entry_checks` accepts N-authored `(NameSite, checked ExprId, RuleId)` checks.
It emits the checked expression and explicit Check at entry, preserving the
contract span and caller/callee cleanup. Source contract elaboration and before/
ensures remain N-owned. The corpus fixture inventory now contains 43 programs,
checked from their original source through the full pipeline, including three
check/call examples and two contract-site failures.

Lowered resource fixtures execute real finalizer bodies on `hirsel-interp` and
assert observable drop order for scope exit, replacement, argument preparation
failure, callee entry, and extracted return values. Cleanup continues after a
finalizer failure while preserving the original failure. Focused assertions cover
control flow, propagation, capture uses and cleanup obligations; representative
printer snapshots cover arithmetic, calls, constructors and entry checks.

`ParameterRef::Body { scope, parameter }` names a checked source parameter.
Lowering resolves it through scoped bindings, including capture slots; an
unbound signature reference in body storage fails closed. No reverse map or
flattened body ordinal is produced or consumed. H binds formal parameter and
result contexts with the shared lexical graph walker. Exported headers retain
Bound depth/ordinal references, including outer names in nested callbacks;
canonical encoding v3 rejects body references.

Named function values become capture-free TIR closure thunks. Their parameters
forward the checked function signature's passing modes and instantiated arguments
to the named definition; indirect invocation uses the ordinary call cleanup path.

Every successful lowering in the unit suite calls
`hirsel_validate::assert_valid` with its owner and callee declaration snapshots,
including resource layouts. Source fixtures use actual declaration queries;
isolated fixtures supply explicit signatures independently of argument elaborations
and emitted TIR. Each fixture owns its declaration map.
The helper asserts every validator relationship, including scoped owner and call
origins. Missing origin evidence is a violation. Copy capture
fixtures retain J's permitted Unique-to-Aliased weakening in the child slot.

Observable record tests verify written field evaluation order independently of
field declaration/drop order: preparing right then left, then releasing right
before left, with real lowered resource finalizers.

`Input.result_types` passes J's `Vec<(NameSite, TypeId)>` formal frame results
in the body's origin namespace. It selects results for the root, closures and
local functions; the table must cover every source frame exactly once.
H's pre-mode FunctionPlan identifies owners; its result rows are not rechecked
against J's refined types. Missing, duplicate or invalid result rows fail closed.

`OwnershipFacts.call_arguments` separates I’s callee consumption decisions from
source evaluation uses. Required rows cover every ordinary call operand exactly
once; missing or ambiguous rows fail the lowering contract. Owned
copies, moves and fresh computations are prepared before applying a borrowed
consumer use, so caller cleanup retains the prepared owner on success and failure.
Borrowed and mutable source places retain their identity.

K emits a dedicated existing unary Unique rvalue for I-selected deep copies
and detach copies. It retains the checked result type and source Copy use; an
already-unique source is reused by Take. Erasing this operation into ordinary
Use(Copy) would incorrectly ask a shared copy to establish unique ownership.
`Resolution.drop_binding` is the resolver-allocated implicit finalizer local.
Body checking consumes it directly; no phase assumes its numeric ordinal.

Lowering consumes shared `UniqueDecision` and `GuardedPatternUse` rows directly.
A unique action determines its source extraction through the shared `usage()`
method; the driver does not reconstruct a second ownership decision.

Lowering emits `Rvalue::Size` for resolver `ResolvedName::Size` rows; it does not
look them up as ordinary scoped local storage. Generic finalizer self rows carry
the declared type/size arguments and finalizer frames retain those generic bounds.

Call lowering uses the checked callable result locality to close evaluation loans
for arguments absent from local(x). Parameter ordinals select retained sources;
source evaluation order stays unchanged. Scoped captured result sources retain
the indirect callee environment. Global results retain no evaluation loans.

Branch lowering joins alternative loans instead of requiring identical loan lists.
Post-join cleanup emits conditional ends for loans absent on a predecessor. Borrowed
temporary transport retains its source loans and nonowning state; it never borrows
the intermediate alias slot merely to transport the view across a join.

Lowering consumes shared CaptureMode rows directly; no driver-specific tuple or ordinal conversion is needed.

C3 lowering emits BindOrigin/EndOrigin to separate source binding incarnations
from compiler storage moves. Tagged accesses retain relative field paths; owned
initializer staging transfers identity even for Plain values. ReadConstant retains
a materialized constant's declared DefPath identity. Capture entry places retain
the source binder name for validation against creation operands. Canonical
signature encoding v7 distinguishes argument identity from ArgumentBrand selectors.
Whole owned replacement ends the destination's name (PROV-1 fresh identity): before
the exchange when the storage is live, after installation when it was dead or only
conditionally live, as after an implicit move of the old value. A binding is a new
owner when its initializer, seen through transparent block tails (#50), is a written
take or a copy of a computed value; the authority pass applies the same test.

Destructuring introduces direct field names together with TIR BindFields before
ordinary child binding transfers. Owned staging fields, including Plain values,
relocate with administrative Take. Borrowed staging fields retain source identities.
Sibling-dependent field types are replayed independently by V against the declaration.

Loan preparation returns a distinct Place access tagged with the new LoanId.
Every call, capture, discriminator, pattern and borrowed transport consumes that
access. Field/index projections and named path reconstruction preserve it.
Mutable loop binders keep their scoped element loan; replacement uses that
permission, and a compound read's temporary reborrow ends before replacement.
Scope cleanup ends descendants before parents.

Borrowed let initializers retain their source place and loans. Only owned
initializers acquire staging slots for pattern transfers; discarded borrowed
views end their evaluation loans without leaving initialized storage in a loop.

Administrative pattern moves never relocate an indexed element. A checked Copy
from a staged sequence remains Copy; consuming loops use their owned cursor
extraction protocol rather than inferring disjoint moves from an index.

`OwnershipFacts.replacements` supplies exactly one owned content TypeId for each
replacement; it is independent of localized read types. Every capture requires a
shared `CaptureMode` row naming its scoped owner. Lowering never reconstructs
these facts from destination storage or a matching binder.

Written take/copy operations lower to evaluated ownership transfers with their
checked value type. Each nested marker consumes the preceding prepared value;
`take copy x` copies x before taking the copy, and `copy take x` takes x before
copying that value. An enclosing marker cannot overwrite an inner extraction.

A scoped capture carries its declaring frame's placement, brand and locality
origins into the child, including nested checked type edges. Lowering preserves
the checked type and does not require duplicate owner parameters there; TIR validation independently checks the capture relationship.

Match lowering transports CheckedBody.patterns into ordered TIR match tables.
H publishes checked literal payloads through its private lowering plan; K adds no
pattern acceptance decision. Guard tests borrow before commit and false guards
restart the same table after cleanup. There is no match fallthrough sentinel.
The independent V matrix proof and direct table evaluator preserve correlated
variant/Boolean products; generated matrices and retained-size gates cover the
cutover. See the R4 match addendum.

I's filtered `OwnershipFacts.identity_joins` supply existing BindOrigin(Fresh) instructions at expression joins and loop heads. Storage and ownership are unchanged. Lowering looks up the published binding and emits the introduction; it does not decide whether aliases diverged.

Lowering transports I's current locality together with uniqueness/sendability when
an owned local receiver changes content. Its receiving storage type remains fixed.

Loans retained by a Local RHS stored into an outer receiver end in the receiver
scope. Repeated reads may reuse an unconditional live loan over exactly the same storage. This keeps
branch and loop getter results usable without repeating a static loan creation.

Whole assignment transports I's receiving availability: live uses strict atomic
Replace, dead uses initialization, mixed uses Replace.if_live and a tied conditional
displaced release. Installation precedes old finalizers in both exchange forms;
receiving loan promotion still follows the store. No dead storage origin is read.

Named constant paths lower directly to operand-free ReadConstant, retaining the
producer's checked type. Initializer literals and constructors occur only in the
constant owner or anonymous comptime materialization. Materialized data rows now
come from hirsel-tir, shared with driver/native snapshots. Named function values
lower to FunctionReference with checked generic arguments; no forwarding body is
created. Capturing local functions keep their ordinary closure bodies. Signature
encoding/fingerprints are v9 (driver query v10/interface 8).
K projects patterns with PatternDecision.ty and transfers into BindingDecision.ty.
Their shape and use must agree; receiving modes may differ from physical storage.
A guarded borrowed binding retains its existing physical projection on later
name reads. Expression receipt modes are transported as value evidence, never
used to retype the aliased field's physical place.

Lowering keeps private sparse content-to-loan dependencies alongside storage
availability. Result selectors and scoped callees retain their operand
permissions; aggregate fields and transports retain the same dependencies.
Dependencies are keyed by part paths in which every element of a sequence, map
or set is one step (OWN-3), as the IR checker keys them, so a view inside a record
in a list stays held until the element's field is read; one element's write
leaves the others' dependencies in place.
Replacing receiving storage updates those holders after atomic installation,
then statement cleanup ends permissions with no surviving holder. Lexical views
and loop controls remain explicit holders until their scopes end. This keeps a
same-source replacement usable and releases a displaced final view before a
later mutation; lowering does not publish new ownership or type judgments.
Loop lowering retains feedback permission state separately from break exits,
including continue edges and condition evaluation. A private CFG pass transports
emitted loan presence to canonical strict/mixed begins and exact failure cleanup;
it does not recreate checked types or uses. Cleanup transport memoizes shared
nodes rather than expanding each root. Source loops retain one emitted body.

K's scoped storage bindings use a primary stack per exact owner/name key, with
only declaration keys retained for guard rollback. Reads and identity-join updates
select the current binding directly; the growing backwards row search is removed.
`work::read(Counter::LoweringBindingRows)` counts lookup work in cold repeated-parameter source tests.

Lowering consumes I's private physical projection type facts independently of
expression receipt types. Dotted fields, explicit fields and indices carry the
published physical row; `Value.checked_ty` carries the receiving judgment for
transport. K does not derive locality or field uniqueness from an expression's
use. The independent TIR checker still computes every projected storage type.
Loan retirement starts from surviving content holders, pending evaluation operands
and lexical loop/view holders. A compiler temporary kept alive solely as a loan
source is reached through those permissions rather than seeding liveness itself.
Ending its last permission destroys the source before releasing captured loans;
cursor storage remains an explicit holder until loop preparation ends.

Indirect Local-result calls retain the callable's installed content permissions
separately from its invocation access loan. Returning captured content through a
body-local closure therefore keeps the actual source bounds without extending the
lifetime of the slot used to invoke it.

K reads I's final-store `OwnershipFacts.plain_types` for cleanup decisions and H's
complete shape plans directly. It does not rescan checked expressions for shapes
or ask the driver to classify the checked type store again.

Named views retain their evaluated index locals alongside their source root.
Those snapshots remain initialized until the view scope ends, including resource
elements; later writes to the original index binding cannot change the address.

Lowering preserves a callable header’s checked enclosing parameter names without
adding runtime captures for type-only references. Unknown scoped names still
require a binding; the scope-mismatch control remains rejected.

Lowering builds sparse borrowed indexes over checked expressions, patterns,
source places, scoped bindings and producer plans once per definition. Child
closure and clause builders share them. Matching rows retain producer order;
first-row and duplicate/missing/foreign-row checks remain at their original
consumers. No syntax-arena-sized table is allocated per frame. The additive
`work::read(Counter::LoweringDecisionRows)` counter counts input rows and indexed lookups for the
source-level let/closure scaling regression.

Lowering's primary live-root representation is an ordered persistent radix tree.
State snapshots share one root; changed membership detaches a short path. Each
immutable node carries its compiled named/temporary cleanup aggregate, preserving
ascending construction and reverse execution order. Existing counted storage
becoming owned invalidates those aggregates. The former BTreeSet and full-root
cleanup fold are removed; explicit subset cleanup retains its existing ordering.
`work::read(Counter::LoweringCleanupRootRows)` counts inspected cleanup input roots, with cold let
and closure source tests bounding the work independently of elapsed time.

Statement retirement compares changed live-tree nodes to its pre-statement
snapshot, so reactivated old slots remain eligible and unchanged slots are skipped.
Active loan IDs use the existing source rows instead of searching instructions.
Scope retirement prunes immutable scope bounds on the same primary nodes; scope
IDs are allocated in lexical preorder and Local.scope stays fixed. Whole live-set
iteration/retention is removed. `work::read(Counter::LoweringRetirementRows)` counts inspected live
and loan-source rows in cold straight-line/nested-block source regressions.
Internal executable contract headers use mode-checked formal rows. Requires
parameters describe entry, while ensures parameters describe continuing exits;
the original exported signature and plain before snapshots remain declared.

Block result transport retains only permissions used by outgoing content and
their temporary-source dependencies. Dead sibling permissions remain in the
exiting scope for ordered cleanup. This moves permissions on surviving source
storage, without moving or extending the lifetime of temporary source storage.

`Plans.materialized` transports evaluated aggregate data as a `Materialized`
producer type store and typed `ValueNode` tree. Leaves are TIR constants or typed named function references; tuple,
record, distinct, variant, sequence and box nodes carry declaration-order fields
and explicit constructors. Lowering checks the root against the existing checked
expression row and imports the graph; it never chooses field types or modes.
A worklist imports and lowers finite materialized trees without a nesting cutoff,
preserving declaration-order fields and parent-before-child type imports.
Owned synthetic data temporaries use Take and ordinary constructor cleanup.
Named reference leaves use the same capture-free forwarding bodies as source
function references, with the imported producer type store and declared passing
modes.

Loop pattern bindings name the yielded value, separately from the collection or
cursor source. Each iteration introduces a fresh binding identity, including
borrowed indexed items. The projected item's existing brand, locality and arena
placement remain unchanged. This lets `string.find(item, ...)` return positions
of that exact item without claiming the collection itself is a string owner.
`PROV-1-for-item-find-run` and its rejected twin retain the same-owner requirement.

Call completion retires invocation permissions through the surviving content
holders. Evaluated temporary owners last through their statement, including
owners named only by result brands. Their captured views retain permissions
until owner cleanup, including mixed borrowed handle and taken payload captures.

Core `replace` is a lowered intrinsic: normal call argument preparation precedes
one TIR `Replace`, and the displaced temporary becomes the call result without
destruction. Assignment uses the same exchange helper, then drops that temporary.
