# Generics

Generic parameters are written `[T]`, `[T: bounds]` or `[n: size]`. Bounds follow
[GEN-1](#gen-1-bounds); behaviour such as ordering or hashing uses explicit
[dictionaries](#gen-2-dictionaries).

```hirsel
fn pair[T: copyable](take x: T) -> (T, T) { (copy x, x) }
fn same[T: data](a: T, b: T) -> bool { a == b }
```

Without `copyable`, `copy x` of a `T` is GEN-1; without `data`, `a == b` is GEN-1.
A third kind, `[H: implementation Sig]`, binds a static family of types, sizes and
operations ([GEN-6](#implementation-contracts)).

### GEN-1: Bounds

Bounds are structural, computed from the type, with no user instances. An unbounded `[T]`
admits every type, which generic code may borrow, move and drop; ordinary copying needs
`[T: copyable]`, except for the signature-derived result-read obligation of
[OWN-1](ownership.md#own-1-borrow), which needs no written bound. A transparent record,
variant or tuple has a bound exactly when all its fields do, with no opt-in, as Rust derives
its auto traits: `type Services { tasks: Tasks, state: Mutex[Board] }` is copyable and
sendable. A `Shared`, `Mutex` or `Atomic` handle is copyable whatever it holds, since a
copy is a count on the root or run-owned cell and never copies the contents.

- Representation: `plain` (copyable and holds no blocks; copies implicitly) and `flat`
  ([MEM-12](memory.md#mem-12-flat-types)).
- Equality: `data` (no functions, capabilities, resources, handles, arenas or raw
  pointers; structural `==`, IEEE on floats) and `key` (`data` without floats; hashable).
- Crossing: `sendable` ([MODE-4](ownership.md#mode-4-sendable)); for `sendable`, "type"
  includes the actual modes.

Inside a function with an implementation binder `H`, a type member `H.T` has exactly the
bounds its signature declares, and generic code may rely on no others. An actual whose
type misses a declared bound is refused at the implementation, citing the bound's rule.
- Authority: `inert` (empty authority summary,
  [CAP-2](capabilities.md#cap-2-charged-at-use)).
- Destruction: `noalloc_drop` promises nonallocating destruction. Primitives qualify;
  aggregates and containers qualify exactly when all their owned components do; a
  resource additionally needs an explicit `@noalloc drop(x)`. Opaque types export this
  bound or conservatively lack it. An unbounded `T` can still be dropped, but not under an
  allocation prohibition without this bound.
- Serialization: `serializable` ([SER-1](serialization.md#ser-1-serializable-schemas)).

An opaque type's bounds and authority summary are part of its signature, and opaque
declarations are checked against their representation in the owner's view, sealed
capabilities included; [CAP-5](capabilities.md#cap-5-capability-sealing) changes
authority abstraction only, not structural-bound proof. There is no universal `==`.
`Shared`, `Slots`, `TypeShape`, `SortedMap` and the planned `Entry`
([RUN-3](runners.md#run-3-entries)) satisfy neither `data` nor `key`, including
inside aggregates; `Map` and `Set` have their specified structural equality
([SEM-5](semantics.md#sem-5-maps)). Finite or regular nominal type graphs have no semantic
depth or width cutoff for proving bounds; only strictly growing polymorphic expansion may
stop, as an operational tooling limit, never as false evidence.

**Serializable.** Schema eligibility and explicit `@serializable` opt-in follow
[SER-1](serialization.md#ser-1-serializable-schemas); the canonical codec is
[SER-2](serialization.md#ser-2-canonical-codec). `serializable` implies `inert` but implies
neither `flat` nor `sendable`, and no other bound implies `serializable`.

**Sendability through generics.** A generic type argument may keep a written sendability
requirement supplied by a type-contract position. A function or callable type parameter
without copying permission transports such a requirement through substitution; this
preserves an unknown receiving refinement and does not make an arbitrary value of that
parameter sendable inside the definition. Copying permission is determined only by the
declared signature, including bounds that imply `copyable` and the borrowed-result read
permission of OWN-1. A parameter with that permission cannot be instantiated with an
actual that carries a nonredundant sendability requirement on a value in its owned
payload, the actual itself included; callable parameter and result contracts are not owned
payload, and `Shared` contents sit behind the ownership boundary. Qualifiers entailed by
the actual's other fixed contracts are redundant. This restriction on implicit qualified
instantiation does not change whether a value is copyable and does not forbid copying a
value whose written type is sendable. An explicitly written `sendable T` constrains that
occurrence independently: a qualified actual occurrence may satisfy it with an unqualified
type argument. Copying such a value is checked by the ordinary ownership rules, and any
later qualified receipt must prove sendability from the resulting value. Every repeated
input occurrence discharges its own requirement. A generic parameter without copying
permission cannot acquire it from a local nested declaration or from another binder's
obligation without meeting the corresponding call obligation.

**Moving without copying.** Inside its definition, a type parameter without copying
permission is linear. The body cannot `copy` one of its values, cannot return a borrowed one
as owned, and has no constant or global of its type. So every value of it the body holds
was moved in from an argument or returned by a callback, and every value it gives out is
moved out. Nesting keeps this: a List, Map, Set, Array, tuple, box, record or variant
holding such a parameter is copyable only when the parameter is, so the body can move parts
in and out of it but never duplicate one. A closure that captures such a value either takes
it, moving it once, or borrows it, and a borrowed capture cannot leave as owned.
[MODE-2](ownership.md#mode-2-uniqueness) relies on this to make parametric results unique.

A generic consumer of unique elements needs a complete producer contract. For
example, under [LANG-2](language.md#lang-2-prefix-law), write both the List and
its element annotation before passing it to the generic consumer:

```hirsel
fn need[T](take x: unique T) {}
fn relay[T](take source: unique List[unique T]) {
  for x in take source { need(take x) }
}
fn example() {
  let source: unique List[unique String] = ["a"]
  relay(take source)
}
```

The fresh literal checks under the complete annotation; `unique List[String]`
alone does not supply the nested element contract needed here. An annotation
cannot make an already shared payload unique: for concrete copyable data that
still shares storage, an explicit `unique value` copy may be necessary before
storing it. The copy retains placement and requires permitted allocation
([MODE-2](ownership.md#mode-2-uniqueness)). No later use revises prefix inference.

A whole-value deep copy preserves the source's fixed nested element type. For
example, `unique source` of a `List[List[String]]` owns its copied blocks alone,
but does not supply a `List[unique List[String]]` element contract. Construct the
receiving list element-by-element when that contract is required:

```hirsel
fn relay[T](take source: unique List[unique T]) -> unique List[unique T] { take source }
fn copied_rows(source: List[List[String]]) -> unique List[unique List[String]] {
  let mut owned: unique List[unique List[String]] = []
  for row in source { list.push(mut owned, unique row) }
  relay(take owned)
}
```

Each inserted copy satisfies the receiving element contract. A fresh literal
under the complete nested annotation is also legal; an annotation on a
whole-value copy cannot add that contract.

`csv.parse_owned` deliberately returns outer `unique List[List[String]]`.
Its consumer, `shared.freeze(take rows)`, requires deep uniqueness of that root;
it does not require `List[unique List[unique String]]`. A generic helper
`fn keep[T](take value: unique T) -> unique T { take value }` can transfer the
whole table with `T = List[List[String]]` without introducing nested modes.

Failed generic bounds are prerequisites of the parameter defaults that depend on them
([diagnostics](diagnostics.md#expression-obligations)): a `sendable` bound failure reports
MODE-4, `inert` CAP-2, and every other bound GEN-1.

A generic function, including a lexical function with captures, used as a monomorphic callable value is
instantiated from its immediate complete expected callable type ([LANG-2](language.md#lang-2-prefix-law)).
Both parameter and result types can determine type and size arguments; every argument
must be uniquely determined, including parameters absent from the signature's value
types. The actuals must satisfy the same bounds as a direct call. Substitution preserves
the declared parameter conventions, modes, authority, allocation and result relations;
an expected type cannot strengthen those promises. `@preserves_unique` remains a
direct-call promise ([MODE-2](ownership.md#mode-2-uniqueness)). Lexical function values
with captures borrow their existing scoped environment and require a compatible
scoped callable context, such as a borrowed function parameter, as nongeneric
lexical references do. Capture-free lexical
functions can become ordinary global callable values. No owning environment is
re-created at instantiation. Each instantiation executes the static lexical body with its determined arguments.

```hirsel
fn identity[T](take value: T) -> T { value }
fn example() -> u64 {
  let f: fn(take u64) -> u64 uses {} = identity
  f(3)
}
```

### GEN-2: Dictionaries

Behaviour is explicit values: `Order[T]`, `Hasher[T]`, `Eq[T]` and user records of
functions. `Order` is never derived. Dictionaries are pure. A container whose invariant
depends on a dictionary is `of` it; a top-level dictionary such as `u32.order` is one
identity everywhere. An implementation ([GEN-6](#implementation-contracts)) is not a
dictionary: it is a static map chosen at each call and is never a value, so it has no
identity a container can be `of`.

```hirsel
fn larger(a: u32, b: u32, by: Order[u32]) -> u32 {
  if by.compare(a, b) > 0 { a } else { b }
}
```

### GEN-3: Reflection

A type has a shape ([CT-3](compile-time.md#ct-3-shapes)) when it is a nominal record or
variant declaration, or a transparent alias of one, and its representation is visible to
the package that writes `shape(T)` ([PROG-2](programs.md#prog-2-visibility)). Scalars,
tuples, function types, builtin collections, `distinct` types and aliases of any of these
have none: `shape(T)` of one is GEN-3. A generic declaration has a shape at each concrete
instance. `TypeShape[T]` carries no structural operation, and taking a shape adds no bound or
operation to the shaped type. Generated function bodies read shapes
([CT-5](compile-time.md#ct-5-generation)); generating declarations is reserved
([reserved compile-time features](compile-time.md#reserved-for-future-compile-time-features)).

### GEN-4: Representation

Generic code never boxes: `T` is laid out as `T` in every instantiation. Foreign functions
and exported C functions are monomorphic. An implementation binder has no runtime operand:
every engine compiles a call `f with (H = m)(..)` as a direct call to `f` specialized at
`m`, and each `H.op` in it as a direct call to `m`'s target, before any optimization.

### GEN-5: Polymorphic recursion

A cycle of calls whose type arguments grow (`f[T]` calling `g[(T, T)]` calling `f[U]`) is
rejected. This is the one program-level check: it runs over the instantiation graph
assembled from each definition's recorded calls and type arguments, and its verdict does
not depend on compilation topology ([PROG-5](programs.md#prog-5-topology)). The diagnostic
cites the earliest type-growing call edge of the cycle. A specialization at an
implementation map is one node of this graph; a function calling itself under the same
map (`f with (H = H)(..)`) is ordinary recursion and never grows.

### GEN-6: Dictionary constants

A dictionary type is a transparent nominal record, neither distinct nor resource,
with callable or `data` fields. A top-level `const` of this type declares a closed
field graph. Every constructor argument is labelled. Callable fields name declared
functions, instantiated from their complete expected field type, or capture-free
function literals. Data fields use ordinary constant expressions, including legal
`comptime` data. Calls returning dictionaries, runtime locals and captures cannot
initialize this declaration. There is no runtime initializer.

Named members retain existing callable compatibility: modes, effects, allocation,
brands and dependent result relations are checked against the field contract.
Capture-free literals retain their own-parameter result restriction. A field does
not acquire a declaration-only `@preserves_unique` promise from its initializer.

Each constructed constant has its own dictionary identity even when all fields
are equal. `const alias: Dictionary = original` preserves the terminal constant's
identity and brands; alias chains normalize before brand checking and cycles are
GEN-6 with the complete chain. Reading a constant materializes its checked field
graph in the interpreter and native backends, including non-scalar data fields.
This declaration form does not permit publishing callable results from arbitrary
`comptime` evaluation: that boundary remains data-only under CT-2.

#### Implementation contracts

A `signature` declares a first-order family: type members with bounds and an optional
authority ceiling (`type State: plain + inert uses {}`), size members (`size bytes`) and
complete operation headers (`fn finish(take state: State) -> Inline[u8, bytes] uses {}`).
An operation header declares no generics of its own. An `implementation` maps every
member by name: each type to a type, each size to a literal and each operation to a
declared function path. It has no initializer, captures or other members.

```hirsel
signature Digest {
  type State: plain + inert uses {}
  size bytes
  fn finish(take state: State) -> Inline[u8, bytes] uses {}
}
implementation sha256_impl: Digest {
  type State = std@1/sha256.State
  size bytes = 32
  finish = std@1/sha256.finish
}
fn finish[H: implementation Digest](take s: H.State) -> Inline[u8, H.bytes] uses {} {
  H.finish(take s)
}
fn digest(take s: std@1/sha256.State) -> Inline[u8, 32] {
  finish with (H = sha256_impl)(take s)
}
```

One matcher checks an implementation against its signature, once, at the implementation.
Members pair by name and kind; a missing, extra or wrong-kind member is GEN-6. Each type
actual meets its member's bounds. Each operation target is instantiated from the header
at the actuals and must stand where the header is specified: passing modes (OWN-1),
types (GEN-6), allocation (MEM-5), authority within the header's (CAP-2), result
relations (MODE-1), brands (PROV-1) and value modes (MODE-2). Contracts must be implied:
a target may not require, nor a header ensure, anything the matcher cannot show (FACT-1);
write an adapter function where implication cannot be shown. Every refusal names the
member.

Implementation binders appear only on top-level functions, after their ordinary
generics. `H.T`, `H.n` and `H.op` name the binder's members; a size member used as a
value is a `u64`. The function's body is checked once against the abstract members, and
its signature records each binder and its members' identities. A call writes every
implementation actual in a `with` clause: a declared implementation path, or an enclosing
function's binder. Omitting one, binding a binder twice, naming an implementation of
another signature, or calling or naming a binder function without the clause is GEN-6.
An abstract operation `H.op` is only ever called.

Two binders denote equal members only where written: `B: implementation Sink where
B.Key = A.Key`. Sharing is never inferred from a body, and a `with` clause whose actuals
break a written equation is GEN-6.
