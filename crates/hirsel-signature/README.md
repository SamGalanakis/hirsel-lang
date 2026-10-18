# hirsel-signature

Normalize written headers and nominal declarations without reading bodies
(LANG-6), retaining self-contained public signatures and private owner shapes.

Allowed production dependencies: hirsel-span, hirsel-intern, hirsel-syntax,
hirsel-types, hirsel-diag and shared hirsel-check-protocol. No body, lowering
or driver dependency, including in tests. The driver supplies resolution, metadata and signature providers.
hirsel-check-protocol owns the existing declared-type helpers shared with body
annotations, and its `fact` module owns private predicate conventions.

`hirsel_signature::normalize(Input, &mut dyn SignatureProvider)` returns an owned
`QueryOutput<Signature>` or rule/span diagnostics. Input (`hirsel_check_protocol::header`) contains the item/tree,
interner, FileContext, shared Resolution, shared Metadata and ordered declaration
sites for the queried owner. Metadata supplies
public visibility and alias status per DefPath, canonical Unsafe/Result roles,
the selected `unsafe ffi` packages and their declared safe wrappers; an extern's
ForeignContract is its ABI and attached doc comment (PROG-4); an extern declaring
`@preserves_unique` is LANG-7, since no body can prove it. These facts are never
read from bodies. Normalization
uses fixed grammar defaults and records all named-type/contract signature reads.
The sibling body checker owns todo marking validation and incomplete-call checks.
`hirsel_types::encoding::canonical_encoding` and `fingerprint` accept a Signature plus its dependency list;
they validate reachable graphs and implement length-framed, domain-separated SHA-256
v2. Keep dependencies with the snapshot when recomputing its fingerprint. Aliases
expand with fresh IDs; nominal references keep DefPaths. Opaque representations
are omitted from public fingerprints. Their public TypeStore also carries no
private rows; normalize_owned retains the defining graph in its separate owner
artifact for the driver's package-scoped body and validation views.
Contract ordinals remain owned by the item.
Revision 1 copies growable field markers and declared destructor allocation flags
into semantic data and hashes them. It also hashes signature visibility, alias,
FFI flags and nominal roles. Type rows are immutable; create a new adjusted row
instead of mutating a stored type. The shared graph importer/substituter handles
binder scopes; this module's wrapper records nominal dependencies and translates
GraphError. Provider snapshots are Arc SignatureRef values.

Normalization computes structural cleanup, including generic DependsOn conjunctions,
nominal recursion and resource finalizer promises. Opaque and erased callable/Iter
cleanup requires an exported proof; invocation noalloc does not imply cleanup
noalloc. Resource authority joins declared fields with explicit uses. Opaque
exports must state bounds (including : ()) and uses; omissions cite LANG-6.
When an opaque declaration supplies an owner representation, every exported bound
must follow structurally and its declared authority must cover the field summary
(CAP-2). Broader summaries are conservative. Represented capabilities instead export their exact nominal authority and retain a
private implementation_authority ceiling. CAP-5 proves that ceiling, exported
bounds and global scope eligibility against the owner representation; capabilities
never prove Inert, Data, Key, Flat or Serializable. Source-less intrinsic/FFI
types retain explicit implementation trust.
Task P checks destructor bodies/field cleanup and growable-field legality.
Scheduling-handle and erased-state authority stays Unknown when the complete
nominal governing authority is unavailable; no capability identity is invented.
Resolver-selected native declarations supply their authority summaries with generic
arguments substituted. This proof belongs to that type occurrence and survives
qualifiers; a missing declaration remains conservative.

The driver indexes declaration sites by DefPath, retaining source order and
cross-file duplicates. Input.declaration_sites supplies the queried owner's slice;
standalone fixtures can supply Metadata.declarations. Item.name_span selects the
current exact name token; the later duplicate reports LANG-7 there. Driver indexing
preserves duplicate candidates. Shared CheckedBody transports typed bindings and patterns,
including unused binders; H/I/J/K follow its documented ownership and use rules.

Signature metadata retains canonical Tasks/Scope roles; signature encoding v5
fingerprints them. Builtin authority uses the shared constructor rule when no
resolver-selected declaration provides a more specific summary.

Signature property derivation reads transitive nominal declarations through the
provider and records those dependencies. A missing cache entry cannot become a
negative sendability proof; the same provider error policy applies to the header
and to representations consulted for structural properties.

SER-1 normalizes zero-argument `@serializable` on transparent data declarations,
checks the complete schema, and retains the written public opt-in independently
of Flat and Sendable. Generic declarations and uses require exact lexical
Serializable evidence. Transparent aliases preserve their target's decision;
opaque/refined domain types use ordinary DTO conversion and validating factories.
Ineligible declarations and generic schema uses report SER-1 before dependent uses.
