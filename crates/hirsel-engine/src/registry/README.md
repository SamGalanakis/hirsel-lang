# Core signature registry

`BuiltinRegistry` is immutable declaration data served through SignatureProvider.
`section22.txt` transcribes the function ledger of docs/spec/core-library.md verbatim. Type/error and
record declarations are explicit data in build.rs. No runtime implementation is
consulted. Every signature carries the canonical v7 fingerprint. `DefPath::core`
and `nominal_roles` provide nominal identities to adapters, with
`hirsel_types::core_roles::intrinsic_roles`;
core@1 is provisional under #36. The driver must first honor own-package lookup,
then use lookup_name for the implicit core dependency. Explicit foreign package
paths cannot fall back to core. The registry revision hashes the whole declared
surface; provider consumers retain that revision even on misses.

Intrinsic roles are typed authority values paired with canonical DefPaths. Shared
thaw, arena.with/bounded and region.with have no special authority rule; their
declared signatures carry their existing constraints.

Intrinsic scalar domains, range/width relations, positive-size ordinals and
cleanup proofs travel in `SignatureMetadata.schema`. The registry owns the ledger;
hirsel-types owns only descriptor types and the body checker consumes its existing
callee snapshot. Ordinary signatures have an empty schema. Schema variables are
identified by declaration ordinal, so their spelling cannot supply Plain or
noalloc_drop evidence. Nominal cleanup uses the constructed declaration snapshots.
The signature fingerprint hashes every descriptor and the registry revision hashes
those fingerprints; empty-schema canonical vectors remain unchanged.

Every domain is checked in calls and annotations using its declared RuleId.
`OMITTED` lists unsettled #4/#6/#7/#16 surfaces; M2 execution may remain
unavailable independently of these declaration facts.

Compiler-only loop protocols are separate from the core library's source API.
`hirsel_types::core_roles::loop_protocols()`
returns `(protocol, open_path, next_path)` rows for `MapInsertionOrder`,
`SortedMapOrder`, `SetElements`, `ArrayConsume`, `ListConsume` and `InlineConsume`.
Unicode operations are selected by the view types' nominal `loop_source` declarations.
Their functions and generic opaque Cursor/ViewCursor types live under `core@1/$intrinsic`.
The `$` segment is not lexable as a source identifier, and provider source lookup
also rejects it. Direct signature queries remain available to driver/lowering.

Open borrows the source and returns a cursor local to it; Unicode protocols use
ViewCursor[element], while SetElements uses Cursor[element]. map opens have
K/V generics, Set opens have a key-bounded T, Unicode opens have fixed element types. Next has one element generic,
a mut cursor parameter and core Option[element] local to that cursor. Its None/Some
ordinals match iter.next. Positions retain source provenance. GraphemeView and unicode.graphemes
load from ordinary std@1 source, with no core row or private grapheme protocol. All protocols declare no allocation, preserve source authority
and drop only their cursor bookkeeping, never borrowed resource elements. The section 14 `forget` intrinsic is also registered with its consuming noalloc signature. These
registry signatures share
the canonical review/hash tests and registry revision. No source next/length API
or TIR advance opcode is invented.

Explicit integer `.order` constants materialize the declared `Order[T]` record.
Its compare field refers to one of ten private, pure noalloc integer comparators.
The field type comes from the substituted declaration, preserving nominal identity
and every type/mode judgment. No structural dictionary derivation is performed.

Erased Iter types retain `Type.erased_captures`. Declared `FromArguments` uses
scoped `RelationPlace { parameter, fields }` selectors; shared instantiation derives known authority and mode
sendability from selected actuals. None remains unknown. Capture evidence survives
generic/scoped graph transport and v5 fingerprints; it is also valid on function values under revision 6.
The validator derives call results independently from their declaration.

`$intrinsic.heap` is a private noalloc producer of the canonical Arena value with a known heap allocator target. `HeapRole` selects its identity; source spellings cannot select private operations.

Consuming Array/List/Inline loops select private ArrayConsume, ListConsume and
InlineConsume protocols. Open takes the source into resource OwnedCursor[T]; next
mutates that owner and returns an owned Option[T]. Its Array[T] representation
preserves element cleanup obligations, so noalloc extraction does not imply
noalloc destruction. These declarations use the shared CursorPlan through H/J/K.

ScalarPositions_open declares its item brand with ArgumentBrand(source), selecting
all retained source brands rather than the PositionView wrapper identity. Ordinary
Parameter origins continue selecting value identity. Shared scoped instantiation
handles both; no per-brand replay loop lives in J. The selector is encoded in v7.

All scalar domains are checked. Calls and Simd/Atomic annotations use the same
shared evaluator and the registry-selected diagnostic identity. SIMD lanes may
be integer, float or bool scalars; SIMD addition requires sized integers.

`operation_inventory()` adds native callable records keyed by path and declaration
digest. Header graphs supply conventions, cleanup and allocation; dispatch tables
supply explicit implementation disposition. Effects conservatively include unknown
reads, writes, ownership, failure and observation. Signature-only application
fixtures and M2 calls are explicitly unavailable. Bidirectional set equality tests
cover both missing handlers and handlers without declarations.

`InlineConsume_open` permits allocation: the native cursor has no size argument,
so it moves the inline elements into one Array block. The List/Array opens and
all sequence consuming steps remain NoAlloc; shared sources use element copies.

Revision 6 publishes nominal borrowed-loop associations and scoped callable state
promises. Core Unicode views use their nominal declarations rather than collection
selector labels. Implementations requiring preservation or creation-envelope proofs
currently reject explicitly as not yet provable.
The wave-2 numeric cutover removes core u64.to_string and both intrinsic float
reduction schemas. Four fixed IEEE bits/from_bits declarations remain core; the
43 concrete numeric algorithms and their shared error types load through the
ordinary std@1 source manifest. There are no std registry slots.

The host I/O ledger declares Stdin/Stderr as global capability types, typed
InputError/EnvError variants, and Args count/text operations. All use the existing
parameter-authority derivation; no ambient-grant or checker exception is added.
