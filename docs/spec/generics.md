# Generics

Generic parameters are written `[T]`, `[T: bounds]` or `[n: size]`. Bounds are structural:
computed from a type's representation, never declared by instances. Behaviour such as
ordering or hashing is passed as explicit dictionary values.

```hirsel
fn pair[T: copyable](take x: T) -> (T, T) { (copy x, x) }
fn same[T: data](a: T, b: T) -> bool { a == b }
```

Without `copyable`, `copy x` of a `T` is GEN-1; without `data`, `a == b` is GEN-1.

### GEN-1: Bounds

Bounds are structural, computed from the type, with no user instances. An unbounded `[T]`
admits every type, which generic code may borrow, move and drop; ordinary copying needs
`[T: copyable]`, except for the signature-derived result-read obligation of
[OWN-1](ownership.md#own-1-borrow), which needs no written bound.

- Representation: `plain` (copyable and holds no blocks; copies implicitly) and `flat`
  ([MEM-12](memory.md#mem-12-flat-types)).
- Equality: `data` (no functions, capabilities, resources, handles, arenas or raw
  pointers; structural `==`, IEEE on floats) and `key` (`data` without floats; hashable).
- Crossing: `sendable` ([MODE-4](ownership.md#mode-4-sendable)); for `sendable`, "type"
  includes the actual modes.
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
`Shared`, `Slots`, `TypeShape` and `SortedMap` satisfy neither `data` nor `key`, including
inside aggregates; `Map` and `Set` have their specified structural equality
([SEM-5](semantics.md#sem-5-maps)). Finite or regular nominal type graphs have no semantic
depth or width cutoff for proving bounds; only strictly growing polymorphic expansion may
stop, as an operational tooling limit, never as false evidence.

**Serializable.** A type is serializable when its transparent data schema has a specified
canonical encoding and a checked decoder. A user nominal type becomes serializable only
through `@serializable`, validated structurally against its complete schema; transparent
aliases keep their target's status. An attribute or written bound cannot certify an
ineligible field. `serializable` implies `inert` but implies neither `flat` nor
`sendable`, and no other bound implies `serializable`.

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

Failed generic bounds are prerequisites of the parameter defaults that depend on them
([diagnostics](diagnostics.md#expression-obligations)): a `sendable` bound failure reports
MODE-4, `inert` CAP-2, and every other bound GEN-1.

A top-level or imported generic function used as a monomorphic callable value is
instantiated from its immediate complete expected callable type ([LANG-2](language.md#lang-2-prefix-law)).
Both parameter and result types can determine type and size arguments; every argument
must be uniquely determined, including parameters absent from the signature's value
types. The actuals must satisfy the same bounds as a direct call. Substitution preserves
the declared parameter conventions, modes, authority, allocation and result relations;
an expected type cannot strengthen those promises. `@preserves_unique` remains a
direct-call promise ([MODE-2](ownership.md#mode-2-uniqueness)).

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
identity everywhere.

```hirsel
fn largest(xs: List[u32], by: Order[u32]) -> Option[u32] {
  let mut best: Option[u32] = None
  for x in xs {
    best = match best {
      None => Some(x),
      Some(b) => if by.compare(x, b) > 0 { Some(x) } else { Some(b) },
    }
  }
  best
}
```

### GEN-3: Reflection

Reserved for future type-shape reflection. There is no executable shape-inspection,
derive or body-generation API. The `shape` keyword and its type-taking form are reserved;
an opacity-aware reflection contract can be specified later without committing to
source-body inspection or a shape representation
([reserved compile-time features](compile-time.md#reserved-for-future-compile-time-features)).

### GEN-4: Representation

Generic code never boxes: `T` is laid out as `T` in every instantiation. Foreign functions
and exported C functions are monomorphic.

### GEN-5: Polymorphic recursion

A cycle of calls whose type arguments grow (`f[T]` calling `g[(T, T)]` calling `f[U]`) is
rejected. This is the one program-level check: it runs over the instantiation graph
assembled from each definition's recorded calls and type arguments, and its verdict does
not depend on compilation topology ([PROG-5](programs.md#prog-5-topology)). The diagnostic
cites the earliest type-growing call edge of the cycle.
