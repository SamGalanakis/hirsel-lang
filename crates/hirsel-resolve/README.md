# hirsel-resolve

Resolve modules, definition names, local bindings and signature dependencies.

Allowed dependencies: hirsel-span, hirsel-intern, hirsel-syntax, hirsel-types, hirsel-diag.

Invariants and boundary:

- Consumes SyntaxFile/item and SignatureProvider; produces Resolution and diagnostics/dependency reads.
- Lookup order and module-head namespace obey LANG-4; package version is part of identity.
- Locals precede use and do not shadow locals or same-module values; local recursion is rejected (LANG-10).
- Expected-type variants may remain unresolved until checking; do not infer through bodies.
- No parser or checker internals. Record unresolved lookups against the manifest revision.

## API and driver integration

`ManifestFixture` is the small replaceable build-input boundary for the package loader
[#4](https://github.com/SamGalanakis/hirsel-lang/issues/4). It contains one current
package, explicit definition metadata for that package and its dependencies,
module-head exposures, and a revision hash. `from_files` obtains definition paths
and visibility from syntax roots; their parser-supplied `FileContext` is already
encoded in each `DefPath`. Add dependency metadata/exposures before constructing
`ManifestIndex`. There is no source import syntax, directory discovery, or file
position in identity. Dependency versions are distinct packages. Duplicate paths
produce LANG-7 diagnostics with both declaration name spans (the specialized
no-overloading obligation, matching signature normalization); malformed canonical
paths cite PROG-1.

The immutable index precomputes module heads at each module prefix. Binding
reservations use ordinary fully qualified lookups in the declaring module; those
positive/negative reads and namespace facts drive cache validation. No package-wide
reservation table or special cache input is needed. `for_package` selects a current
package while sharing immutable tables.

`resolve_item(file, item, symbols, index, provider)` resolves one definition at a
time. This makes lexical IDs independent of top-level declaration order. The
provider supplies shared immutable SignatureRef snapshots; the resolver never reads another item's body.
Same-file written type/function headers also supply nominal expected context. Cross-file representation information comes
only from provider signatures. A provider may supply builtin names not present in
the index; its lookup must enforce visibility for metadata it alone owns.

`ResolveOutput.query` carries `QueryOutput<Resolution>`. Revision 1 moved all
these tables and their record types into shared Resolution; ResolveOutput derefs
to it for compatibility. The driver must retain the owning syntax snapshot:

- `reads`: every attempted manifest lookup (including reservation probes and
  misses), module-head lookup, and successful/failed signature query. Lookup/head
  reads carry the explicit manifest revision. Signature reads carry an optional
  fingerprint; `None` means the query failed, not that it can be omitted from the
  cache key. Provider diagnostic failures are retained too. `query.dependencies`
  contains successful signature fingerprints; `Resolution.dependencies` contains
  sorted, deduplicated definition paths, not unsuccessful name spellings.
- `types` and `generics`: primitive/type-parameter uses and generic
  declarations with ordinals in the entire generic list (e.g. `[T,n:size,U]`
  means type 0, size 1, type 2). Built-in bounds arrive as the shared
  syntax enum; there is no independent string-based bound vocabulary.
- `contextual`: constructors, variants, and fields (including labelled constructor
  arguments, destructuring, projections, and leading-dot sibling brands). A
  variant/field ordinal is paired with its stable nominal `DefPath` owner.
- `paths`: uses, brand, placement, locality, and error-map references which lack a
  key in shared `Resolution`. Dictionary references use the same `DefPath` through
  exposed heads and full versioned paths (GEN-2); dictionary purity is checking.
- `local_functions`: ordered statement-to-local bindings. A local function becomes
  visible after its body is resolved. Self/later-local references cite LANG-10;
  top-level recursion is allowed.
- `reference_scopes`: the declaring function for value-parameter references.
  Generic references carry their declaring NameSite directly in BinderRef::Body;
  `generic_declarations` stores the declarations at those scopes. A closure's
  parameter zero and an enclosing parameter zero remain distinct. These sites and
  all other syntax IDs are intra-definition data, never persistent identities.
- `deferred`: known bare variant names or projections needing checker-supplied
  expected context. `resolve_item_with_expected` accepts expression/pattern/error-mapper hints
  for these uses. Expected context comes from written annotations or signatures;
  it is never inferred through a definition body. Unknown ordinary names still
  diagnose LANG-4. Bare error mappers defer until their expected error context is
  supplied; that context is distinct from a Try expression's success type. The
  checker owns refutability, exhaustiveness, arity, generic
  substitution, and whether a context admits a constructor.

Diagnostics preserve AST origins. Header names, labels, generic entries, uses
paths and variant names recover exact sites through SyntaxFile token-origin
helpers. Hand-built trees without a tape retain the containing-origin fallback.

Matching core/std module paths share one head; all other distinct targets remain
ambiguous before member lookup. Missing-member and ambiguity receipts retain the
versioned candidates, so diagnostics and cache validation observe the selected heads.
Core declarations are ordinary metadata entries supplied by the engine.

The index enforces package privacy and `internal` ancestry. Public signatures
cannot mention indexed private types, including nested generic/function types.
Public opaque representations are hidden outside their package; their nominal
names remain usable. Local bindings cannot shadow any live local or a
same-module top-level value. Locals may shadow values from other modules, including
the root fallback. Module heads
are a separate namespace and take precedence before a dot.

## Verification and corpus responsibility

`//:hirsel-resolve-unittest` runs hand-built ASTs and fake providers, with no
implementation-crate dependency. A 256-case proptest permutes declaration order
and compares `Resolution` by stable definition path. The resolver-owned corpus
programs (the LANG-4, LANG-10 and PROG-2 rejections, their accepted twins, and the
PROG-1 and GEN-2 acceptance and identity examples) run end to end in the
conformance gate, which checks each header's rule and line.

ResolveOutput.operational_failure preserves a provider operational failure even
when recovery produced diagnostics; the driver propagates it through QueryError
and never caches the recovery as deterministic rejection. Provider lookups carry
LookupOutput.manifest on every result, recorded separately from the local index
revision. ExpectedTypes and ManifestFixture remain resolver-owned input APIs.

Type-parameter and primitive uses populate ResolvedName in the shared `types`
reference table; consumers use that single table.
Unqualified compiler-native type constructors can resolve without an indexed
library definition; their missing lookup reads are retained. No nominal identity
or expected shape is invented, and an indexed/provider definition takes priority.
Manifest duplicate diagnostics use Item.name_span directly, without tape scans.

`Resolution.drop_binding` is the resolver-allocated implicit finalizer local.
Body checking consumes it directly; no phase assumes its numeric ordinal.

`resolve_view` selects an explicit `ResolveView`: `File(item)` permits written
same-file header fixtures, `Item(item)` permits only the owner header and body,
and `Header(item)` excludes execution expressions. Item/Header shape fallback
can inspect only the queried owner; sibling declarations use recorded provider
reads. Headers retain contracts, distinct facts and finalizer binder declarations.
All views preserve the original syntax IDs and spans without modifying the tree.

Issue #4 now has an explicit JSON loader. `ManifestFixture`
remains the resolver-owned metadata input, not a filesystem loader.
`ManifestIndex.with_exposures` shares declaration tables while selecting one
importing package's dependency heads; the driver supplies that view.

Type-item authority paths recognize the item's own type parameters before nominal
lookup. Unknown names still diagnose LANG-4; callable/value path resolution is unchanged.

`@loop_source` operation paths and the `@format` function path record manifest lookups and resolved definition
identities without reading operation signatures. Those signatures may name the
declaring receiver; querying them during header resolution creates a cycle.
The second declaration stage validates every association after headers publish,
including associations unused by a loop.

Phase fixtures can select dependency signature snapshots with
`ManifestFixture::with_dependency_signatures`; their declaration metadata, visibility
and top-level heads participate in the ordinary resolver index.

A failed dotted module lookup that also names a local explains the module-head precedence and suggests renaming that local. User types cannot claim primitive or compiler-native constructor names; LANG-7 cites the declaration.
