# hirsel-check

Check definition-local bodies, contracts, types, bounds and generic instantiations,
and audit mode-checked source allocation and representation promises.

Allowed production dependencies: hirsel-span, hirsel-intern, hirsel-syntax,
hirsel-types, hirsel-diag and shared hirsel-check-protocol. Body annotations
reuse its declared-type rules; produced plans belong to that shared protocol.
The driver composes checking, modes and
[hirsel-lower](../hirsel-lower/README.md); checking never depends on lowering.
The existing driver dev-dependency supplies registry signatures to internal body
harnesses (the exception in `tools/buck2/gating.py`).

Every expression has a checked type/use decision. All returned TypeIds belong
to CheckedBody.types. No callee implementation is consulted. Record instantiated
call edges for the driver's topology-independent GEN-5 graph check. Signature
normalization belongs to [hirsel-signature](../hirsel-signature/README.md).

`body::check_with_loop_protocols(body::LoopInput { body: Input { tree, item,
context, resolution, signature }, protocols }, provider)` returns
`Result<body::Output, QueryError>`, containing checked rows and lowering plans. Inputs are one
retained syntax snapshot and its declared signature. Recovery returns typed rows
plus sorted rule/span diagnostics; callers must reject diagnostics before modes or
lowering. Operational and unavailable provider failures propagate for retry.
Every attempted signature query is retained in `CheckedBody.resolution.reads`;
successful signature dependencies also appear in `QueryOutput.dependencies`.
Callee implementations are never queried. All returned TypeIds belong to
`CheckedBody.types`, imported/substituted using the shared graph helpers.

An explicit Enter/AfterChild/Finish stack visits expressions in evaluation order.
Each item carries expected type and a lexical function-frame index. Nested
closures and local functions have independent parameter/result/error channels;
return, propagation and recovery cannot change another frame's owner. The syntax
resolver's `reference_scopes` survives unchanged.
Closure literals retain a contextual `@noalloc` callable promise even without a
literal attribute. The allocation audit checks their bodies and cleanup under
that promise before publication (CLO-2, MEM-5).

H emits one expression elaboration per visited expression, one scoped binding
row per declared parameter/local/local function, and one pattern row for every
pattern including unused binders, wildcards and recovery; every alternative of an
or-pattern has its own binding row for the local the first alternative declares,
and usefulness expands alternatives the way it splits constructors. Pattern field ordinals
refer to declaration order; rows retain source order independently. Generic
instantiations use caller-owned actual types, preserving complete mode/sendability
evidence. Dictionaries remain explicit ordinary call arguments. I/J enrich modes,
uses, authority, origins and captures; H emits none of their diagnostics. A
function type written in a body keeps its written `uses` at every depth (#58),
lowered as a signature's is: a definition is a capability and a binding adds
its type's authority. J still supplies a top-level binding annotation's
reception restriction.

`LoopProtocolPaths` is plain registry data: `(&'static str, DefPath, DefPath)`
for each protocol label and its open/next definitions. Production checking has
no driver dependency. Cursor loops require their declared open/next operations;
missing declarations fail closed. Tests take the protocol table from
`hirsel_types::core_roles` and the declarations from the driver registry through
a dev-dependency. Cursor plans contain the complete checked calls; ordinary Iter
loops carry their declared next operation directly.
Lowering consumes these producer decisions after I/J:

- `CheckedBody.calls` records definition/value/nominal constructor targets,
  source-order argument ordinals and requirements, generic actuals and recovery.
- `CheckedBody.places` records scoped storage roots, checked field/variant
  ordinals and index operands, with the source access kind. Lowering reads these
  rows directly; it performs no field-name lookup or private-plan conversion.
- `TryPlan` records the registry-selected Result identity, its actual success/error
  variant ordinals, the enclosing Result/error type and error ordinal, and an
  optional named function or variant mapper. No tag position is guessed.
- `ForPlan` records the selected `ForProtocol` and either one complete
  `ForPlanKind::Cursor` row or a direct range/indexed/advance execution plan.
  Cursor source use, element type and Option step layout belong only to its
  shared `CursorPlan`; J refines that row without maintaining mirrors.
  Direct plans retain their statement, checked element and written source use.
  Indexed plans carry declared length identities or an Inline size plus its u64
  judgment; direct Iter advancement carries its declared next and Option layout.
  Cursor mappings select provider-backed open/next paths and generic actuals.
  Both GEN-5 edges and successful/failed provider reads remain recorded;
  declaration-relative origins/locality are refined by J. Missing consuming
  cursor declarations fail closed. No implementation body is read.
- `FunctionPlan` records each frame's owner and inferred or declared result.
  Nested closure results belong to their frame; local function signatures never
  infer from the body.
- `Plans.shapes` maps each shape expression and syntax type to a caller-owned
  semantic TypeId, aliases followed, for lowering's shape rvalue. `shapes.rs`
  admits a shape only in compile-time code (CT-3), of a record or variant
  declaration (GEN-3) whose representation the signature view shows (PROG-2).

Missing nominal capability arguments retain typed ordinary inputs/results and
are diagnosed by authority checking under docs/spec/diagnostics.md, rather than a derivative arity
error. Resource finalizers type their implicit local binder, enforce unit results,
and reject propagation with the specialized DROP-3 diagnostic.

Test-only lexer/parser/resolver dependencies parse source text;
`tests/library.hir` transcribes the core library signatures. The registry uses
manufactured nominal capability/error types and never supplies a library body.
Conformance programs run end to end in the corpus gate rather than here. The body
suite holds hand-built arenas/manufactured signatures, parsed programs, a
20,001-expression stack-depth case, and a 128-case property test comparing repeated
checks after symbol insertion, provider loading, and signature type-graph insertion
changes. Cursor tests cover owning-protocol declarations, actual Option tags,
missing/partial mappings, unavailable signatures, malformed declarations and
deterministic plans after symbol, mapping and signature-store reordering.

`audit::check` consumes source syntax, the declared signature and the mode-checked
body after J/I and before lowering. It rejects invalid growable fields and
variable-size flat indexing/replacement, importing nominal representations with
shared substitution and recording every consulted signature dependency. It
produces source diagnostics; the driver alone selects their order under docs/spec/diagnostics.md.
Stable flat byte encoding remains issue #1 and is not inferred from target ABI.

Contract clauses receive ordinary body elaborations, calls and provider reads in
 their lexical function frame. `hirsel_check_protocol::body::Plans.contracts` carries checked roots and
plain `before` occurrences; lowering snapshots those on entry and checks requires
there, and checks ensures against an extracted result on every return. Contract
loans and temporary owners end after each check. These are ordinary TIR Check
instructions with the declaration span, so expression failures keep their own
rule and site. The checker enforces boolean clauses, signature-derived purity,
plain snapshots and before-only postcondition references to taken parameters.

The allocation audit consumes J's `call_allocations` and I's allocating copy and
mutation decisions as plain slices. It checks every executable branch, nested
function/literal frame, contract call and owned binding under a noalloc promise;
borrows create no cleanup obligation, and an unconditional transfer is exempt.
A call's promise is the one its callee's row carries: a declared signature, a
written function type, or the `@noalloc` evidence J publishes on an inferred
literal's row; the audit infers none of its own. Escaping constructions are traced
back through the initializers of bindings the walked frame owns (a captured
binding was initialised in another frame); explicit arena/region destinations
remain noncounted. A binding's cleanup obligation is its row's cleanup summary,
which J fixes for a closure literal where it is built. A temporary has the same
obligation where its frame destroys it: a discarded statement value, the rest
of a wildcard or `..` pattern, a lent argument or projected base, a branch
condition or scrutinee, and an operand that a later operand of the same call,
construction or operator may fail before the consumer owns it. A binding read,
a constant and a literal's borrowed captures destroy nothing (MEM-5). H now
computes structural destruction summaries for newly constructed body type rows.

Inline layout auditing follows nominal representations with generic arguments
substituted, rejecting cycles at their field annotation under MEM-7. Boxes and
block containers break the inline graph; zero-capacity Inline storage has no child
edge. This is a source layout judgment and leaves TIR validation unchanged.

`hirsel_check_protocol::fact::fact_signature` supplies the private predicate convention from the
declaring type signature, and `hirsel_check_protocol::body::Plans.facts` retains its checked bool type,
borrow convention and instantiated edge. Lowering uses those facts to call and
check the predicate before constructing the nominal value. The signature checker
requires a plain base for `where` and hashes rooted parameter projections by their
field spellings, without querying them as definition paths.

Generic graph substitution takes the declared `NominalRoles` and normalizes
`FunctionType.error` from its substituted result, including nested callbacks.
The shared `result_error` helper recognizes Result by nominal identity. Pure
imports preserve existing channels; body checking needs no post-substitution repair.

Generic inference preserves context-derived and nested declared type arguments.
Value-derived arguments use shared `generic_type_argument`; every complete actual
occurrence is separately recorded in `CheckedBody.calls` for bounds.
Inference never overwrites a contextual argument with a qualified actual value.
Pending call expectations containing an uninferred type or size parameter are
withheld until actual argument types bind those parameters.

An expected success type flows through `?` as a registered Result context with
an error variable scoped to that try expression. Operand signatures, constructor
payloads and contextual callback results resolve the error independently of the
frame's channel. Known nested type shape stays exact; published decisions contain
resolved actuals, and propagation retains its LANG-8 prerequisite diagnostics.

Signature and body checking use shared declaration-based Plain, Copyable and
Sendable proofs from hirsel-types. Generic substitution derives property summaries
through the shared normalizer; concrete Plain callback parameters use Global
borrowing defaults. Bounds still inspect complete actual generic evidence.
H uses the rooted `shape_equal` relation without manufacturing an erased copy of
its type store. The shape relation remains intentional: I/J own mode/provenance
source diagnostics, while V checks complete published value relations.

Erased Iter types retain `Type.erased_captures`. Declared `FromArguments` uses
scoped `RelationPlace { parameter, fields }` selectors; shared instantiation derives known authority and mode
sendability from selected actuals. None remains unknown. Capture evidence survives
generic/scoped graph transport and v5 fingerprints; it is also valid on function values under revision 6.
The validator derives call results independently from their declaration.

Heap checking reads the canonical private producer signature and emits a zero-argument checked call. Lowering executes that row through normal calls; no unit placeholder remains. Signature encoding v6 includes heap roles and allocator refinements.

Computed unary expressions receive fresh expression origins. Only written take,
copy and mut access operators retain their operand identity; unique and box remain
value-producing operations even when their storage implementation reuses a value.

Inferred closure results derive their Result error channel when H finishes the
literal. J derives it again when provenance refines the inferred result graph;
the result and error always describe the same callable contract.

Construction context retains explicit root placement without copying expected
brands or stronger mode promises into actual rows. Nominal constructors, tuple
and sequence construction, and block literals use that context; unqualified
nonplain fields inherit the owner's placement through the shared field helper.

E and H use `structural_flat` for the complete Flat bound. Signature normalization
still owns the distinct inline-field compatibility judgment for opt-in flat layout.
MEM-12 excludes capability/handle/resource/callable storage and requires actual
Flat layout evidence for nested nominal and generic fields; Plain alone cannot
establish it. String/Bytes remain legal inline fields in an opt-in flat owner.

Nested generic direct calls retain their inferred actuals in CheckedBody.calls.
Lowering transports those actuals for indirect callees too. Syntactic size
arguments in nested headers are size evidence, never recovery type rows.

NoallocDrop instantiation checks the complete cleanup summary, including hidden
owned components. MayAllocate cannot be proved from visible type arguments.
DependsOn is resolved directly against lexical NoallocDrop/Plain bounds, without
reconstructing parameter graphs recursively.

Registry schema constraints may require an existing language Bound at instantiation.
They name a TypeParameterRef from the exact declaration binder and carry the
diagnostic rule; E/H reuse their ordinary proof with that actual type. Encoding
and independent validation reject foreign Body/depth/ordinal references.
SortedMap's retained borrowed key provisionally requires Copyable (spec #48);
section 22 ledger signatures remain verbatim.

H publishes `CheckedBody.tails` beside calls and places. Source evaluation and
lexical owner lifetimes determine self-tail eligibility; scoped bounds and nominal
declarations prove Plain bookkeeping. Single-use Data temporary borrow arguments
carry their source expression and declared ordinal for internal loop ownership.
Lowering transports these decisions without inferring eligibility from CFG shape,
and relocates already evaluated owned temporaries without repeating source copy.
Pending resource/release/loan obligations and postconditions retain ordinary calls.
ConstructorHole is reserved until spec issue #46 settles its continuation policy.

Borrowed and mutable Map/SortedMap loops publish indexed entry plans and a whole-source loop loan. Keys and whole entries are read-only; destructured values may be mutable. Consuming map loops publish Take-only owning cursor calls with owned Option entries. Lowering transports these plans, preserving insertion/key order and cleanup on break.

Each requires/ensures clause has an H-owned, capture-free all-Borrow callable
plan and a distinct parameter scope. Ordinary parameter references, snapshots
and the virtual result map to explicit clause formals; lexical generic references
retain their original declaring scope. J/I check those roots independently of
the ordinary body. K emits one clause producer per clause and calls it at entry
or every normal exit, with FACT-1 checks at the clause span. Snapshot children
remain original-frame entry expressions and are evaluated exactly once.

A bare record value must have a complete generic context and no required fields.
H reports missing generic context at the name under LANG-2, then missing labelled
construction under LANG-12, following aliases through declared representations.
Direct constructor callees still infer from their arguments; shape hints impose no
value obligation. Empty records and contextual nullary variants remain values.

Until spec issue #57 defines a first-class range value contract, H permits a range
only as the direct source of a for statement. The statement publishes that source
before checking it; no whole-file scan is needed. Other contexts report SEM-4 at
the range and recover without a fabricated Iter value or dependent literal errors.
Independent endpoint expressions still receive their own prefix-derived types.

Unknown or duplicate call labels publish unmapped recovery arguments. Their
context-dependent literals receive the recovery type, suppressing derivative
LANG-2 reports. Operand walks still check independent types and explicit numeric
suffixes, including range/format errors. Valid mapped operands and nested generic
calls retain their own prefix inference; every invalid mapping remains Recovery.

Branch checking retains prefix context: when there is no explicit expected type,
a checked nonbottom then branch or earlier match arm supplies the type for later
branches. Guards retain bool context. A later arm cannot type an earlier literal,
and invalid or returning arms supply no new context. Private continuations keep
this check in source order without revisiting arm bodies.

Allocation auditing carries a construction destination through global taken
arguments and arguments selected by an escaping retained result. A NoAlloc callee
signature proves invocation, not the caller's new argument storage. Borrowed
arguments and discarded results keep frame-local construction when eligible.

Expected box types supply their payload contract, as expected tuples supply their
parts. The operand may carry stronger uniqueness evidence; I checks any explicit
unique payload requirement at construction, separately from the physical type.

H rejects a nonrecord type name in value position with LANG-7 at the name
(spec #64). Record constructors keep their field and generic-context checks;
callee and shape probes do not impose the bare-value category check.

Inline comptime eligibility is checked against resolved lexical bindings before
lowering (CT-2, spec #65). Its private evaluation entry has no enclosing runtime
parameters or contracts; nested callable frames retain their normal inputs.
The check walks sorted source intervals rather than rescanning the expression
arena for each wrapper. Abstract type mentions are rejected at definition time;
size parameters are staged per selected instantiation. Closed expressions inside
generic definitions remain valid. Result types must prove Data from declared
signatures before lowering, even in unselected definitions; Never and recovery
types retain evaluation-failure behavior.

H publishes source places through a primary expression-keyed map during checking.
Projection, access, contract and tail queries use that same map; CheckedBody's
canonical place rows are collected once at publication. The mutable working
vector and its repeated search/sort path are removed. `work::read(Counter::BodySourcePlaceRows)`
counts real lookup/publication work in cold closure source regressions.

Executable clause planning preflights its formal type graph before rebinding any
occurrences. Invalid exit signatures that name a taken parameter produce no
clause frame; authority checking owns the brand/placement declaration diagnostic.
Borrowed owner identities remain ordinary clause formals.

Allocation audit distinguishes scalar acquisition from counted deep copies.
Unique actions follow the source's owned components, including nominal and Plain
generic proofs, and stop at Shared handles. Empty owned graphs and explicit
arena/region destinations do not introduce a MEM-5 copy site.

Body source names, reference scopes, local functions and elaborations have primary
first-row indices. The indices select arena positions, so contract rewriting still
reads current names and owners, and mutable elaboration updates retain their first
match. Index construction and lookups contribute to `work::read(Counter::BodySourceRows)`.

The source allocation/representation audit indexes immutable producer positions
once. Place, call, pattern, closure, name, scoped binding and let-statement
queries retain first-row/source-order policy and read the original producer
values, avoiding repeated scans during payload checks and local effect proofs.

Shared declaration-property helpers return QueryError, including the declared
operational limit for nonregular polymorphic recursion. Signature and body
snapshot-only adapters retain that failure through their publication boundary;
a conservative local fallback cannot turn it into a GEN-1 diagnostic. Finite and
regular property graphs have no nominal expansion depth limit.

The allocation audit also consumes H/J's `ForPlan` decisions. Under `noalloc`
it reads every implicit open, next and indexed-length declaration's allocation
effect, and the checked cursor type's drop summary. This covers consuming
sequences, Map, Set and Iter, including loops in nested functions and closures;
rejections point to the loop source, like written calls.

Bytes reads produce u8; mutation and mutable loans reject LANG-5. A discarded scalar projection is still materialized so bounds checks execute. Wave-1 std uses ordinary source package bodies and the existing callable/ownership surface.

Revision 6 publishes nominal borrowed-loop associations and scoped callable state
promises. Core Unicode views use their nominal declarations rather than collection
selector labels. Body-local callable types normalize state selectors against their
own parameter binder and declaration-order fields, as headers do. A closure literal
receives the state promises and the result's `@captures` selection of its expected
callable type (kept relative to its own binder on the closure's type), and inside a creation
owner a literal at a call's mutable callback slot receives a preservation promise on
that slot; ownership and V prove each closure body against its type.
Whole-input Iter creation
promises are proved from actual operands by Source and V; field-selected and
callable-result creation retain their conservative guards.

Generic inference keeps written refinements supplied by a type-contract occurrence.
A concrete type argument drops the Environment access marker of a borrowed callback
parameter while retaining its crossing requirement, uniqueness and explicit placement.
This lets one T describe both a generator result and its borrowed consumer input.

Literal kinds with a single type (bool, String and Bytes) supply that intrinsic
type, including the first branch of a match or conditional. Numeric literals and
bare variants remain contextual under LANG-2.

Call completion proves Serializable for every inferred actual before generic value normalization, including actuals inferred solely from the expected result (codec.decode). Parameter occurrence checks alone cannot cover this case.

Shape mismatch diagnostics print both expected and actual source-shaped types, with iterative graph traversal and cycle detection. Bytes loops yield u8 through checked bytes.len and index operations; mutable Bytes iteration is rejected before publication.

Contextual variants use the complete expected type before global-definition fallback, including nested generic constructor arguments. The selected constructor replaces stale resolver context and is published to the later phases. Local and parameter lookup retains precedence.

Generic callback inference removes the borrowed parameter's Environment access
relation for abstract type parameters as well as concrete types. A callback's
borrow of T cannot change a separate callback's promised T result; named source
relations and explicit placement remain part of the inferred contract. Nominal
type arguments and taken callback parameters retain their declared abstract
relations.

F-string steps (SEM-3, `body/formats.rs`): an `Append` checks its value with no
expected type, then its type selects `std@1/text.append` for `String` or
`std@1/<scalar>.format_into` for `bool`, integers and floats, or the function a
nominal type's `@format` association names, with the written
`(value, mut builder)` mapped to declared ordinals; `Finish` is
`std@1/text.finish_valid`. The callee is then typed and recorded in the body's
resolution as a named reference to that operation, so modes and lowering see an
ordinary call. Another value type, or a program without std@1, is SEM-3; for a
nominal type the message names `@format`.

Specified appends select `<scalar>.format_into_spec` or `text.format_into_spec`,
with six parser-supplied options between the value and builder. Radix specifiers
require integers, and an `@format` type takes no `AppendSpec` options (SEM-3).
`Mark` is `core list.len` of the builder; `Pad(hole)` is `text.pad_number_from`
for an integer or float hole and `text.pad_from` for `String`, `bool` and `@format`
holes, and recovers silently when the hole's append already reported. Ordinary
call checking checks every option, including a `name$` width's path (LANG-7 for
a non-u64).

Each function frame retains its active loop statements and labelled blocks. Labelled exits require a matching enclosing label and active loop and block labels cannot shadow; nested callable frames start empty (LANG-7). A labelled block's type is its expected type, else the first typed break operand or tail; the tail is checked against it once known. A value break naming a loop is LANG-1; `continue` naming a block, and an unlabelled exit whose innermost loop lies outside an enclosing labelled block, are LANG-7. Tail eligibility treats nothing inside a labelled block as a tail and joins each break's state at the block's end; the allocation audit treats a labelled block as exposing its bindings and destroys a break operand exactly when its block's value is destroyed (LANG-11).

Generic named function values infer every type/size actual from the immediate complete
monomorphic callable expectation. H checks structural bounds and records the
instantiation at the name for GEN-5 and existing FunctionReference lowering. Lexical
references publish their actuals on the expression elaboration and preserve their
capture-bearing source row. It
substitutes the declared callable contract rather than copying receiving promises.
Missing context or an undetermined parameter reports LANG-2 naming the parameter.

RUN-3: where `Entry[F]` is expected, a path naming a non-generic, non-extern, complete
top-level function forms an entry when the definition has F's shape and stays within F's
allocation contract and `uses`. The entry is typed `Entry[F']`, F with the definition's
parameter and result types, so a generic call infers its actuals from it; a written F
spells default effects, which a definition's derived evidence never equals. Any other
expression there, and a generic definition, is RUN-3 at the expression. A call argument
whose template is an Entry gets its unknown generics as fresh inference variables before
the argument is checked. It lowers to the existing FunctionReference.
Callable binding annotations keep the declared parameter/result header, including
omitted mode defaults, separately from the initializer's actual callable. Receipt
checks cannot silently inherit stronger uniqueness or locality promises.

Repeat-literal elements receive the expected sequence element type through
ordinary `copy` expressions. Their one-element desugaring defers Inline length
agreement to O, which has the evaluated count; ordinary literal lengths remain
checked by H.
