# hirsel-types

Own semantic types, complete signatures, checked decisions and shared query data.
Allowed dependencies: syntax, intern, span, diag, codec; no implementation crates.
Wire tests use run-config's shared `hex` formatter as a dev dependency.

- `TypeInterner` is the session's one canonical, concurrent type interner
  (design §1a). Every phase and thread interns into it exactly and losslessly
  (`intern`), so one row has one `TypeId` and promotion cannot fork identity.
  `TypeId(u32)` has no provenance and no order; ids never reach output.
  Rows live in address-stable chunks over an optional immutable `TypeBase`
  (the std image), indexed by 64 hash shards. `intern_graph` admits rows
  naming each other before they exist (hand-built and invalid cyclic graphs).
  Per-id memos: `validate`/`valid` (missing rows, recovery rows, cycles),
  `sem` (callable parameter names erased, erased captures sorted, Known(Unknown)
  as None), `strip_modes` and `shape`. `semantic_eq` is `valid` and equal `sem`.
- `TypeMemo` (structural, session-wide, one each for I and V) memoizes
  `substitute` by root, binder, arguments, authorities and roles; `instantiate`
  by the view's exact `ViewDigest` and actuals; `project` by (owner, part,
  plain). `ContextMemo` (one per query, bound to its `GenericDeclarations` and
  signature lookup) memoizes `prove`, `plain` and the directional `compatible`
  relation, and replays each entry's lookups on a hit.
- Persisted forms use canonical per-definition tables (`closure`,
  `encode_table`, `decode_table`): first-encounter order from the roots, never
  interner numbering. `TypeId` serializes only inside `with_table_encoding`/
  `with_table_decoding`, as a table position. The std image's one table grows
  as its values serialize (`TableBuilder`), and its rows become an immutable
  `TypeBase` (`TypeTable::into_base`) whose positions values decode to
  directly (`with_base_decoding`).
- `GeneratedActualKey` is main's CT-5 key, the CT-3 closed spelling of each
  actual, interned once and memoized per TypeId.
- `Signature` is self-contained. Fixed header defaults never come from bodies
  (LANG-6). `SignatureMetadata` carries visibility, transparent-alias status,
  the `unsafe ffi` package fact, a safe wrapper's trusted kind and model, and nominal
  Unsafe/Result roles. Opaque representation details, including whether the hidden
  representation is an alias, are private. `FunctionType.foreign` is an extern's
  ForeignContract (PROG-4). Manifest `Metadata` (with each selected `unsafe ffi`
  package's wrappers)/`DefinitionMetadata`/`DeclarationSite` are shared inputs.
- Private `UniqueSummary` rows also carry whole borrowed-data parameters whose
  content does not escape the invocation. The module inference query publishes
  these facts with its reads; callers perform a summary lookup. They do not
  change callable types. Source V and physical C check the body obligations,
  and linked C settles the transitive dependency equations independently.
- `Type.drop_allocation` is independent of function invocation allocation:
  NoAlloc, MayAllocate, or a conjunction of generic NoallocDrop obligations.
  Unproved erased cleanup defaults to MayAllocate. Resource
  `TypeDefinition.destructor_allocation` is MayAllocate unless explicitly promised
  NoAlloc; that promise covers body and field cleanup. `Field.growable` is layout
  metadata. Normalized bounds remain a vector after syntax clause validation.
- Modes, passing, placement, brands, authority and destruction are separate.
  Authority/brand alternatives and obligation lists are sorted and deduplicated.
  `AuthoritySummary::join` unions declared fields and explicit resource uses;
  Unknown cannot prove inert. A finalizer inherits that authority, or a represented capability’s private
  implementation ceiling, never exports body-inferred authority. A whole
  capability contributes only its nominal declaration identity; generic actuals
  and private ceilings never widen it. Global kind alone proves no structural
  bound. The v20 normalization follow-up installs these policies.
- `import_type`, `substitute_type` and `semantic_equal` operate on explicit rooted
  graphs, remapping every type edge. Substitution arguments belong to the target
  store; authority substitutions are supplied from declarations. Nested generic
  functions establish fresh binder scopes. Scoped parameter graph transport is shared; J supplies checked actual identities and authority. GraphError is data, not a source diagnostic. Equality assumes
  normalized sets and exact binder references; it is neither alias expansion
  nor subtyping. Rust Eq compares snapshots, not separately allocated meanings.
- `Resolution` includes expression/type/binding/dependency rows plus reads,
  contextual, paths, local_functions, deferred, generics, generic_declarations and
  value reference_scopes. Contextual ordinals have nominal owners; value, type and
  size parameter references carry their declaring binder. `types` carries primitive and type-parameter
  uses as ResolvedName::Primitive/TypeParameter. IDs require the retained
  syntax snapshot. Failed signature reads use fingerprint None, never disappear.
- `CheckedBody` has expression, binding, pattern, checked-call, instantiation and capture
  decisions. Every binder and pattern, even unused/wildcard, gets a unique row.
  BindingDecision names its owner and source site; PatternDecision maps children
  to declaration-order fields. A let's root PatternDecision carries the
  `WrittenModes` its annotation writes; no later phase reads the annotation syntax. All TypeIds belong to CheckedBody.types. H supplies
  types/elaborations; I/J refine use/mode judgments; K consumes them. Resolver
  LocalId and hirsel-tir LocalId are distinct namespaces.
- H retains actual mode-bearing expression types and instantiation arguments
  before parameter weakening (MODE-4/#29). Every repeated generic argument
  occurrence is checked independently. A sendability proof never grants unique
  mutation through an unannotated parameter. Capture-at-creation use is explicit
  and differs from the use inside a closure.

SignatureProvider returns Arc `SignatureRef`; lookup returns `LookupOutput` with
a manifest revision even on an error. `DependencyRead::ProviderLookup` can retain
that result separately from resolver-index reads. Missing module members and ambiguous
heads retain their versioned candidate DefPaths in LookupError, so diagnostics and
cache receipt replay observe the same selected namespace. QueryOutput carries successful
signature dependencies; record failures outside the Result as well. QueryError
separates diagnostics, unavailable implementation and OperationalFailure. Only
diagnostic errors are deterministic rejection-cache candidates; operational
failures must retry and must not become source RuleIds.

Canonical semantic rendering belongs to `encoding`, shared by signature fingerprints,
returned-type observations and named-function specialization arguments. The body
checker adds source contract code through the same Encoder. Revision 1
used encoding/hash domains v2. Revision 2 advances encoding/hash and driver interface/query domains to v3 for lexical parameter references. Encode all
semantic metadata, nominal role paths, growability, cleanup summaries, destructor
promises and NoallocDrop; omit spans, Symbols, provenance, unreachable rows and
the stored fingerprint. Sort sets, preserve declaration order and traverse roots
rather than arena indices. Contracts include code and used signatures. Private
opaque representation stays excluded. Never use Rust Debug/std::hash as encoding.

Contract ordinals belong to their signature. TypeDefinition.drop names its resource
owner, not a synthetic source-position definition; closure BodyIds remain local. Unit tests include wrong-store IDs, diverging clones,
binder-safe substitution, rooted equality and unused/reordered patterns.

`Resolution.drop_binding` is the resolver-allocated implicit finalizer local.
Body checking consumes it directly; no phase assumes its numeric ordinal.

Generic graph substitution takes the declared `NominalRoles` and normalizes
`FunctionType.error` from its substituted result, including nested callbacks.
The shared `result_error` helper recognizes Result by nominal identity. Pure
imports preserve existing channels; body checking needs no post-substitution repair.

Rooted semantic equality ignores function parameter binder names while preserving
call labels, passing conventions and complete parameter types. Stored diagnostic
names are unchanged; semantic equality removes names only in its private row comparison.

- `UniqueAction`, `UniqueDecision`, `GuardedPatternUse`, `CaptureContext` and
  `CaptureMode` are shared phase decisions. `UniqueAction::usage()` defines the
  extraction mode once; lowering consumes the same row ownership produced.

- `generic_type_argument` removes root access modes from value-inferred
  arguments; explicitly declared nested type contracts retain their modes.
  `ClosureInfo.result_evidence` records the complete checked body result
  independently of its contextual callable result promise.

`property_tests.rs` and `parametric_tests.rs` check `prove` (Plain, Copyable,
NeedsRelease, Sendable in its structural, content and after-sharing forms, Flat),
`structural_equality`, `parametric_inputs` and `unique_rows` against naive references
written from GEN-1, MODE-2, MODE-4 and MEM-12, over random graphs with modes, handles,
nominal declarations behind a stub lookup and every bound set. J, I and V all read
these kernels, so their agreement cannot see a fault in one; the references can.

Structural properties have one declaration-based implementation:
`structural_property` proves Plain, Copyable, Sendable and NeedsRelease;
`structural_sendability` derives conditional generic summaries. Transparent
nominals substitute their actual arguments into their representation; opaque
nominals require exported bounds. Phantom arguments of Ptr/FnPtr/SlotId/TypeShape
are not owned payloads. Concrete locality excludes Sendable, even for flat
values. Function and erased Iter capture summaries remain explicit producer facts.
`projected_type` applies owner locality and the derived root sendability to
nonplain parts, preserving nested declared contracts.
`normalize_type_properties` rebuilds immutable graphs after substitution or mode
refinement and applies the Plain canonical form to every row its declarations
prove plain, structurally or by a bound of the binder the row names.
Provider callbacks retain each phase's dependency/error policy.

Data/Key structural equality also borrows owner rows and imports only reachable
nominal actuals into private scratch storage. Its nominal fixed points and
per-path expansion-depth guard are independent of unrelated sibling applications.

`compatible_type` shares Exact, Value and Parameter relations. Root value modes
may weaken, root brands may erase or join known alternatives, and callable
promises may narrow authority/uses and allocation. Nested identity remains exact,
except that a nested callable is judged by the same promise subsumption in the
direction it is read (`Variance`, CLO-2): tuple, record, variant and box parts,
Array/List/Inline elements and a callable's result and error keep the
direction, a non-`mut` parameter reverses it, and a `mut` parameter and every
other type argument are exact. A row along such a path that is Sendable is
received where sendability is not promised.
The comparator never repairs actual evidence before validating it. Signature
specialization and producers derive summaries before publication instead.


`ParameterRef::Bound { depth, parameter }` names a lexical signature binder;
`ParameterRef::Body { scope, parameter }` names a retained checked source scope.
Authority, origin, argument locality and caller destination use this same identity.
`map_parameter_references` distinguishes root qualifiers from a callable's binder;
`bind_parameter_scopes` preserves inner binders while closing free references.
Call origin and callback authority instantiation traverse every nested type edge.
`OriginRoot::Expression` names a materialized temporary expression within the
retained source snapshot; it is not exported signature identity.

Canonical Tasks/Scope identities are `NominalRoles.tasks_capability` and
`scope_capability`, supplied through Metadata. `builtin_held_authority` provides
the intrinsic constructor summary; phases join retained payload authority.
Missing roles and absent erased Iter evidence remain Unknown. A held handle summary
never provides the governing capability operand of an operation.

Erased Iter types retain `Type.erased_captures`. Declared `FromArguments` uses
scoped `RelationPlace { parameter, fields }` selectors; shared instantiation derives known authority and mode
sendability from selected actuals. None remains unknown. Capture evidence survives
generic/scoped graph transport and v5 fingerprints; it is also valid on function values under revision 6.
The validator derives call results independently from their declaration.

`refine_inferred_value_contract` refreshes H's unresolved free origin/capture
references from a checked generic occurrence. Concrete and unknown declared
capture contracts stay intact; nested binders cannot select outer arguments.

Ambient heap values carry `Type.allocation_target = Some(AllocationTarget::Heap)` independently of their storage placement. `HeapRole` supplies the canonical Arena and private producer operation. Shared destination instantiation respects callable binder depth; root value compatibility can forget but cannot invent this refinement.

`CaptureMode` shares a closure expression, semantic u32 capture ordinal and declaring NameSite across authority checking and lowering.

`CursorPlan` owns the implicit open/next declaration identities, generic actuals,
source use, cursor type and `OptionLayout` in one checked-body graph. H produces
it, J refines it, and K consumes it; the driver never converts its fields.

`OriginArgument` keeps a selected operand's identity and actual locality separate.
`instantiate_parameter_origins` substitutes brands and joins the selected actual
lifetimes through nested callable binders. A concrete local name does not turn a
Global heap value into a Local one. Inferred identity refinement changes only
identity; it cannot invent a lifetime from an origin spelling.

`OriginArgument.brand` carries the selected actual value's retained brand, distinct
from its own identity. `OriginRoot::ArgumentBrand` is a declaration selector used
by position-view cursors to retain their source String brand. Instantiation expands
all alternatives, retains field suffixes, and respects nested callable binders.
It cannot be used as an arena identity. `map_type_origins` provides the common
graph traversal for independent origin replay. Signature encoding is v7.

`instantiate_sibling_origins` replaces sibling selectors with supplied field
identities throughout the shared type graph, including arena placements and nested
callables. Inserted declaration parameter references shift under nested binders.
Missing field identities preserve the unresolved selector; no brand or placement
restriction is erased.

`instantiate_parameter_sources` derives dependent result placement from the
complete actual argument TypeIds while instantiating their identities and
lifetimes. Equal placements survive; Heap + Arena(a) keeps Arena(a). Different
scoped placements become Inferred + Local; consumers retain every selected source
loan, and the result cannot satisfy a specific arena requirement. Nested caller
placement references shift under callable binders.

`placed_part_type` supplies an unqualified block field's construction placement
from its aggregate. Plain and explicitly placed fields keep their declaration.
Read accesses additionally apply `projected_type` for OWN-8 locality.

`SignatureMetadata.schema` carries registry-declared scalar domains, range/width
relations, positive-size ordinals and explicit cleanup constraints. Descriptor
types contain no library identities; ordinary signatures use an empty vector.
Every domain carries its RuleId and is checked in calls and annotations through
`schema_failure`. Nonempty schemas
participate in canonical fingerprints; empty-schema vectors remain unchanged.

`shape_equal` compares only reachable type constructors, nominal arguments, generic
binders, labels, passing conventions and layout flags. H uses it before later
mode, origin, placement, cleanup and callable-effect judgments. It validates
reachable edges and ignores unrelated rows; strict semantic equality is unchanged.

`shape_equal_with_holes` additionally admits caller-designated inference variables
at any nested edge while comparing every known constructor. Both rooted graphs
still reject recovery rows and structural cycles; it neither binds variables nor
changes the strict comparator used for published types.

`structural_flat` proves complete root Flatness through declared snapshots and
actual arguments. It is independent of field storage compatibility and fixed-size
layout. String/Bytes roots fail; explicit flat layouts and opaque Flat exports
qualify, and ordinary nominal values require pointer-free Plain representation.
Tuples, records, variants and `Inline`/`Simd` arrays opt into nothing, so they need
the same Plain proof: an inline array of a `flat type` owning a String is not flat.
Transparent aliases preserve the represented value's proof.

Plain alone cannot prove Flat because it permits raw pointers and SlotId handles.
SlotId remains Plain, Copyable and Sendable, but its scalar bits do not prove
Flat (MEM-12); nominal wrappers and opaque exports cannot add that proof. Ordinary nominal
representations must also prove their components pointer-free, and generic/opaque
Flat obligations need an explicit Flat promise.

`structural_equality` proves Data/Key from declared owned representations and
actual generic arguments; proof scratch never appends incidental rows to a checked
body. Capabilities, resources and function-bearing graphs cannot acquire equality
from a payload-only traversal. `structural_authority` specializes complete published
field/uses summaries without reopening resource fields or reading finalizer bodies.
Both helpers use exact `GenericDeclarations` binder references and caller-owned
snapshot lookup. Recovery or missing declaration facts remain conservative.
`bounds_imply_inert` recognizes Data/Key/Flat/Inert, while Plain alone retains
conditional authority. Checked value-parameter evaluation stays with J.

Checked calls are keyed by ExprId and carry the target, generic actuals, declared
result requirement and explicit completion/recovery state. Arguments stay in source
order; each complete argument includes a formal ordinal and specialized Parameter
whose TypeId belongs to the body store. H publishes the rows; J may refine generic
actuals alongside instantiation evidence; I, audit and lowering consume the rows.

`CheckedBody.places` carries one `SourcePlace` per elaborated source expression.
Roots identify scoped bindings, definitions or computed values. Projection steps
retain field/variant ordinals, index ExprIds and the node performing each step;
dotted path steps share their ExprId. `SourceAccess` records read/copy/take/mut/unique.
H publishes the rows during typing; invalid bodies may contain recovery roots.
Consumers interpret the same paths for loans, value identities, layout auditing
and lowering. Indexed value identities remain distinct from sequence storage;
all indexed storage of one sequence aliases for OWN-3.

`TypeKind::graph_edges` is the complete, ordered structural graph view, including
callable parameter/result/error types and nominal/builtin type arguments. Each
edge identifies its role and value/type/size binder transition. Graph
validation and inferred-value refinement share this enumeration; provenance,
cycles and declaration lookup stay with their callers. Owned-payload proofs,
execution walks and immutable graph remapping retain their separate policies.

Ptr/FnPtr bits are Plain and therefore Sendable under MODE-4. Their phantom
pointees/signatures contribute no owned payload; raw pointers still fail Flat
and Data/Key proofs.

Contract revision 3 uses `BinderRef<P>` for value, type and size references:
`Bound { depth, parameter }` in signatures and `Body { scope, parameter }` in
checked graphs. `GenericDeclarations` resolves bounds at that exact binder.
`Substitution.binder` selects the only binder being instantiated; nested callable
binders shift free references even when they have no generic declarations.
The encoding, signature, registry and query domains are v8; driver interface is 6.

Registry schema constraints may require an existing language Bound at instantiation.
They name a TypeParameterRef from the exact declaration binder and carry the
diagnostic rule; E/H reuse their ordinary proof with that actual type. Encoding
and independent validation reject foreign Body/depth/ordinal references.
SortedMap's retained borrowed key provisionally requires Copyable (spec #48);
section 22 ledger signatures remain verbatim.

Rooted semantic equality is `semantic_eq`: both graphs valid and equal `sem`
normal forms, each row normalized once per session. `shape_eq` compares `shape`
normal forms the same way. `ContextMemo::compatible` keeps main's directional
walk for the pairs a policy can weaken and compares the rest by receiving-contract
normal forms; test-only copies of main's walks (`reference`) check both on every
unit test and on random graph pairs. `visit_type_origins` retains the origin
rewriter's traversal and binder policy without constructing output rows.
Substitution interns its result rows into the session interner, memoized in a
`TypeMemo`; proofs expand nominal representations the same way.

`CheckedBody.tails` records H's source tail decision, kind and counted argument
ordinals/source expressions. It carries decisions rather than proof evidence;
lowering transports them and TIR validation reconstructs its own declaration and
flow proofs.

`TypeKind::map_entry_fields` exposes the canonical Map/SortedMap entry layout as
[key, value], requiring exactly two type arguments. Projection, readonly-key,
mode and generic-bound judgments remain with the consumer.

Revision 4 stores every child collection within a `Type` row in `SharedSlice<T>`
(`Arc<[T]>`): structural children, function parameters/generics/contracts, variant
fields, bounds, brands, authority terms and qualifier obligations. Row cloning
shares unchanged children. Mutating an unpublished producer row detaches only the
edited slice; interned rows are immutable. Construct from a Vec or
iterator, borrow slices for consumers, and use `storage_identity` only for retained
allocation accounting. Identity never affects equality or canonical encoding.

`GenericDeclarations` shares immutable source-scope rows across snapshots and
proofs. Empty declarations have no lookup row. `signature` and `from_binders`
construct exact binders; `insert` builds source declarations. `nested` shares the
source map and adds only a nonempty signature layer, while every callable shifts
relative Bound depths. `with_copyable` adds isolated proof permissions without
copying source declarations. Lexical ancestry validation still visits all scopes,
including empty and duplicate nongeneric scopes. `work::read(Counter::CopiedGenericDeclarationRows)`
counts actual declaration copies for deterministic driver scaling assertions.

`canonical_type_encoding` and `canonical_argument_encoding` render rooted graphs
with the same visitor as signatures, excluding arena IDs and unreachable rows.
They reject free Body generic/value/origin identities rather than encoding source
ordinals as cross-engine meaning. Specialization arguments preserve kind and size.

MODE-2 Shared-root uniqueness is inert (provisional spec #69). Shared graph
normalization canonicalizes a written Unique qualifier to Aliased while preserving
locality, placement and payload contracts. Atomic sole-holder ownership is checked
by runtime thaw. Deep aggregate uniqueness still stops at Shared boundaries.

`parametric_inputs` reads MODE-2 parametricity off a generic signature: when the
owned result holds only values of type parameters without copying permission
(GEN-1), it says, per parameter, whether the argument is free, must be unique, or
is a callback whose instantiated result must prove uniqueness (`unique_rows`: written
`unique`, or a tuple of such or plain parts). A borrowed data input is free when the
callee has a checked body; a lent callback is free when it is `uses {}` with plain,
parameter-free result, error and `mut` parameters. Non-data holders of those
parameters, counting only a nominal declaration's parts that name them, disqualify the
signature, as do borrowed inputs of a bodyless declaration and other lent callbacks.

`compatible_view_type_in` applies the existing non-writing view comparison with
Parameter or Value root policy. Readonly/taken payloads can forget nested Unique
evidence along owned edges; Value still preserves arena lifetime requirements.
Mutable callable once/many multiplicity stays exact; owned reception may weaken
many to once. Mut hand-back uses `compatible_handback_type`: the callee writes the argument
back, so the root's callable promise and every nested row are exact. Wherever a
value is only received, a nested row is also received with fewer erased captures
than the expected row declares (unknown admits any; a `FromArguments` promise
only itself) and with a cleanup summary where `MayAllocate` is expected, as the
root is (`erased_state_promises`, `cleanup_promises`). Callable headers,
phantom arguments and interior-mutable/channel contracts remain invariant; nominal
arguments need a Data proof before their nested evidence may weaken.

Nominal properties use coinductive instantiation memoization: an active repeated
instantiation assumes the property, while all other owned components still need
to prove it. Finite and regular graphs have no width or depth budget. The shared
`NominalExpansions` helper separately bounds strictly growing polymorphic
recursion at 128 active constructor-growth expansions, reporting
`QueryError::Operational` with operation `type-proof-tooling-limit`.
Homeomorphic embedding detects growth even inside a fixed outer constructor;
shrinking actuals, argument permutations and finished siblings do not spend it.
`enter(root, context, store)` takes the store holding each TypeId. It hash-conses
every reached row once into a structural class (the edge-erased row as
`semantic_equal` normalizes it, plus its children's classes) and a height, so the
memo is a class lookup and only a same-declaration active instantiation no taller
than the new root is checked for growth: nested instantiations cost linear work.
Property-dependent contexts (inherited uniqueness or ownership state) remain
part of the memo key. All property-dependent helpers and their declaration lookup
callbacks return QueryError, so a tooling limit cannot become false evidence.
Boundary tests cover 128/129 siblings and finite depths, a 200-level regular
graph, repeated recursive instantiations, and the operational growth boundary.

`rows::Rows<T>` is a fixed-size persistent row container with no flow judgments.
Snapshots share radix nodes; writes copy one path. Callers supply idempotent joins,
and union requires a neutral default. Optional copy counters preserve phase-local
scaling measurements. Producer and independent checker judgments remain separate.

`expand_representation` shares successive nominal layout substitution, with an
optional step count and phase-owned declaration lookup and argument authority.
`RepresentationError` retains the last row and distinguishes missing declarations,
opaque or recursive representations, graph errors and query failures. Consumers
retain their own conservative fallback and must propagate operational failures.

`TypeKind::value_parts` selects structural children without entering callable
binders. It includes nominal/builtin type arguments; owned-payload policy remains
with each caller. `Relation::union` and `Modes::join` preserve equal current facts,
including conditional summaries. Authority's joins first widen non-Global locality
and non-Sendable summaries; adoption must retain that existing normalization.

`Type::with_kind(kind)` constructs an unqualified global row with nonallocating
cleanup and no crossing requirement, allocator refinement, captures or brand.
Callers retain responsibility for derived modes and cleanup obligations.
`Type::unit()` and `Type::boolean()` construct native control rows with Plain,
sendable, nonallocating metadata and require no declared signature read.

`OwnershipFacts` is the shared phase transport I produces and K consumes:
call argument uses, initialized identity introductions and their field projections,
final-store Plain classifications, physical projection types, uniqueness,
capture metadata, guarded pattern uses,
allocating mutations and replacement storage contracts. Slot availability stays
private to I.

`Type.sendability_requirement` records a fixed receiving refinement for every
type and part, independently of `Modes.sendability`, the current content proof.
`WrittenModes.sendable` selects a binding's explicitly written crossing axis.
Receipt checks actual proof before weakening permissions; counted sharing or
changed content invalidates it. Graph transport preserves the requirement without
manufacturing actual evidence. Structural normalization rebuilds ordinary data
summaries; `canonical_current_type` canonicalizes independently replayed flow facts.
Function and Iter evidence describes retained environments independently of root
access locality. A view retains referent proof, while full crossing still checks
its loans and placement bounds. Mutable hand-back compares receiving contracts;
readonly reception accepts stronger proof with binder-aware implication. Semantic
encoding v11 includes requirements; native layout, ABI and instance keys erase them.

Generic qualification transport derives from the declared signature, including
Copyable implications and selected borrowed-result reads. Owned-payload queries
stop at callable headers and Shared boundaries and respect opaque declarations.
Read-only forgetting follows covariance; invariant and mutable contracts remain
exact. The policy adds no signature effect or serialized field. Owned receipt
paths expose qualified physical parts to the two independent flow engines.

Revision 6 publishes nominal borrowed-loop associations and scoped callable state
promises. Core Unicode views use their nominal declarations rather than collection
selector labels. Source and V independently prove whole-Iter preservation and
whole-input creation envelopes. Selected fields and executable callable state
promises retain conservative guards.

Revision 7 adds TypeDefinition.serializable as public explicit schema opt-in and TypeDefinition.implementation_authority as the owner-only CAP-5 ceiling. Whole-value authority stays nominal in every view. Public encoding excludes the private ceiling and includes opt-in/Serializable bounds: encoding v12, signature hash v11, driver query v12/interface11. Constructors default to no new evidence; Source and independent V prove seal ceilings and serialization eligibility.

Revision 8 adds TypeDefinition.format, the function an `@format` association names
(SEM-3), encoded after the loop association: encoding v13. Stage two
(`hirsel_signature::protocol::validate_format`) checks its signature.

`rows::JoinCache` and `Rows::join_cached` are additive pure-container helpers.
One cache fixes one idempotent join operation and default-row domain, memoizes
ordered radix-node pairs, and retains both operands so allocator pointer reuse
cannot forge equality. Computed results may reuse equal incoming subtrees.
The cached join also returns exact row-change evidence, avoiding a second full
comparison walk. The cache is private analysis scratch, dropped with its solve; it supplies no
ownership rule and imposes no neutral-default or commutativity assumption.

`instantiate_cleanup_bounds` resolves cleanup conjunctions throughout a rooted graph
using only NoallocDrop or Plain bounds of each exact declaring binder. Nested
callables establish their own binder scope; all other evidence is preserved.

`serialization::serializable_schema` proves SER-1 and returns a versioned graph
of codec shapes, schema-local references and checked generic template parameters.
It requires explicit nominal opt-in, preserves transparent aliases, and opens only
transparent data representations. Native layout, value ownership and schema
eligibility are independent. Brands, authority, resources, opaque/refined types,
callables and unsupported runtime constructors fail closed at every depth.
`SignatureMetadata::recursive_group` is a recursive type group member's key; the
encoding writes it as a framed extension only when present, so other signatures'
encodings are unchanged. `SignatureProvider::recursive_group` supplies a group's
other members at a `GroupStage`; providers without groups return none.

`structural_serializable` is the boolean form. Source consumers use checked
snapshots; V independently replays admission rather than trusting this descriptor.
Serializable generic evidence implies Inert and supplies no crossing proof.
A nominal instantiation reached again, by recursion or along another path,
references its first node. A declaration marked serializable whose authority is
only its type parameters (core `Option`, `Result`, `Bounded`; std
`ReadError[E]`, `store.TxFailure[E]`) is admitted with unbounded parameters; each use proves its arguments. A core `Path` is a
nominal node over `String`, its text; it has no representation of its own.

`wire` implements SER-2 version 1 over concrete `serialization` schema graphs.
Its explicit envelope binds codec version and the complete canonical descriptor;
no incoming schema supplies source eligibility, nominal equality or authority.
Values are an engine-neutral flat arena, and encode/decode walk iteratively.
Canonical NaNs, UTF-8, lengths, ordinals, insertion-order Maps, byte-sorted Sets,
duplicate keys and trailing input are checked before publishing decoded data.
Caller budgets return operational `Limit`, independently of malformed input.
`encode_fixed_flat` admits only the proven fixed FlatFormatV0 common subset and
validates before copying. Native variable layouts and ordinary ABI bytes have no
such proof. `tests/wire/` holds 44 golden vectors, malformed twins and an
independent Python reader/writer; `tests/wire.rs` runs it in the crate tests.
The codec is a shared data API; source-language generic encode/decode bindings
are a separate library integration, not privileged constructor hooks.

`core_roles` holds the role tables the driver's core library registry and the checkers
share: `LOOP_PROTOCOLS`/`loop_protocols()` (the borrowed-loop protocols and their
`$intrinsic` open/next paths) and `Intrinsic`/`intrinsic_roles()` (the authority
role of each core operation). The registry declares the operations; these tables
are the only source of their roles, so check and modes tests read them without the
driver. `LoweredIntrinsic` enumerates nominal core declarations whose calls lower
into existing TIR instructions; they have no runtime dispatch or operation-manifest
row. Lowering and the operation inventory consume the same classification.

The opt-in `testing` feature exports `testing::Types`, the hand-built type-store
fixture used by the LIR, LLVM and runtime test suites. It derives no property evidence.

`snapshot` owns the canonical Primitive and BuiltinType encodings through
hirsel-codec; the codec itself never depends on semantic types.

Signature encoding v14 retains caller-attribution metadata as a declaration
property. Function value types do not carry that property.

Signature encoding v15 (hash domain v12) includes size parameters on constant
signatures. Written constants remain monomorphic; inline generic comptime staging
publishes private size-generic declarations after the enclosing signature. Type
parameters are excluded from constant signatures.

Signature encoding v16 adds `TypeDefinition.serializable_check`: an opaque type's
`@serializable(check = f)` publishes its check function and its representation, the
wire contract its schema is a nominal node over (SER-1).

Stage-selected analyses live in `hirsel-type-analysis`: cleanup summaries and
specialization, MODE-2 parametric uniqueness, logical arena sizing, diagnostic
spelling, and successive nominal representation expansion. Import those modules
directly. Core definitions, canonical CT spelling, graph transport, nominal proof
memoization and basic property/crossing queries remain here. Cleanup-bound graph
normalization remains with the shared property normalizer.

`core_roles::table_key_parameters` identifies key argument positions of the
std@1/table surface. It supplies operation identities, never a schema proof;
source checking and V independently inspect the declared representations.

`SharedPath::node_allocation_bytes` reports the current tail allocation including
Arc counters and alignment, or zero for an empty path. Walk `prefix` and deduplicate
`node_identity` for retained-byte accounting; payload-owned buffers are separate.
