# Language

This part covers how expressions are typed and evaluated, how names resolve, what a
signature fixes, and the forms of types, errors, tests, recursion, blocks, arguments,
patterns and bindings. The grammar is in [Syntax](syntax.md); locality, uniqueness and
ownership are in [Ownership and modes](ownership.md).

### LANG-2: Prefix law

Every type judgment is decided by the text to its left plus the expected type. Type
arguments are never written in expressions, so a postfix `[` is always indexing.
`shape(T)` names a type in an expression only to read its shape at compile time
([CT-3](compile-time.md#ct-3-shapes)); it is not a type argument.

Boolean, byte-string, string, character and byte literals intrinsically have types `bool`,
`Bytes`, `String`, `char` and `u8`. A character literal such as `'a'` has type `char`,
and a byte literal such as `b'a'` type `u8`, even without an expected type; an
incompatible expected type, such as `u32` for `'a'`, is LANG-2. A numeric literal without a suffix or a bare variant takes its type from the
expected type or, in `a..b`, a binary operator or `==`, from the other operand; with neither,
it is an error. A size parameter
used as a value is a `u64`. Parameters are in scope only after their declaration, and
local definitions precede their use.

```hirsel
fn literals() -> u64 {
  let a: u64 = 3          // expected type
  let b = a + 4           // the other operand
  let c = 5u64            // a suffix
  let ready = true       // intrinsically bool
  a + b + c
}
```

When a bare constructor has a known expected variant type but its name is absent,
LANG-2 names the missing variant and expected type and lists the declared variants:
`Fault is not a variant of FileError; variants: FileFault, Canceled, NotGranted,
NotFound, PermissionDenied, AlreadyExists, IsDirectory, NotDirectory`. With no
unambiguous expected variant type, the diagnostic remains `bare variant needs an
unambiguous expected type`.

`let n = 3` with no annotation, no suffix and no other operand is LANG-2 at the literal.

A generic function name, including a lexical function with captures, may be used as a value when its immediate,
complete expected monomorphic callable type uniquely determines every type and size
argument from the declared signature. This includes annotated bindings, arguments, record
fields and returns. No later statement or implementation body participates. Without that context,
or with an undetermined argument, report LANG-2 at the name and name the undetermined
parameter. Lexical instantiations follow ordinary closure capture rules. The instantiated callable
retains its declared modes, `uses`, allocation and relation contracts
([GEN-1](generics.md#gen-1-bounds)); ordinary callable receipt checks still apply. An
error-mapper path under [LANG-8](#lang-8-errors) is instantiated from the error it maps.
A variant constructor call without an expected type selects an unambiguous variant
declaration in scope and infers its type arguments from its payload arguments'
already determined types, left to right, as for a generic function call. This applies
to user variants as well as core variants: `Some("raw.bin")` determines `Option[String]`.
A local callable or a contextual variant keeps its ordinary precedence. Multiple
user declarations of the same variant name require an expected type; payload
types never choose between declarations. A user variant shadows a core default.
Every type argument must be determined by the payloads or an expected type:
`None()` alone and `Only("raw.bin")` for `type Choice[T, U] = Only(T) | Other(U)`
are LANG-2 because a type argument is undetermined. A later argument cannot give an
earlier unsuffixed literal its type. As in Rust's constructor call checking
([callee.rs:1108](https://github.com/rust-lang/rust/blob/1a2c4797ef2570bd76ca5dee34449169bc7dc457/compiler/rustc_hir_typeck/src/callee.rs#L1108)),
payloads constrain the selected constructor's declared parameters and result.
Hirsel reuses its ordinary generic call inference and does not introduce full
unification or payload-based overload selection.

An ordinary nonempty sequence literal `[e0, e1, …]` keeps a usable complete
expected sequence type: that type fixes its kind, element contract, placement and
Inline size. A conflicting or erroneous explicit context is never discarded.
Only when there is no expected type, check `e0` without an expectation. If it
supplies a complete, valid, non-bottom type T, construct the registered core
`List[T]`, regardless of a user declaration named `List`. Apply ordinary aggregate
and type-argument normalization: infer no root `unique` or written `sendable`
requirement from a value occurrence, preserve nested contracts and brands, and
manufacture no placement. Check the remaining elements once each, left to right,
against that fixed T. A mismatch is LANG-7 at the receiving element. Never recheck
the first element, skip an invalid or undetermined first element, widen T, coerce
values, or solve constraints from later elements or uses. Existing prefix rules
within `e0`, such as other-operand numeric inference, still apply.

```hirsel
fn sequences() {
  let sources = ["-"]             // core List[String]
  let counts = [1u64, 2]           // core List[u64]
  let array: Array[u64] = [1, 2]   // expected kind wins
  let inline: Inline[u64, 2] = [1, 2]
  let empty: List[u64] = []        // empty still needs context
  let repeat: Array[u64] = [1; 2]  // repeat still needs context
  // let a = [1, 2u64]            // LANG-2 at 1
  // let b = [1u64, true]         // LANG-7 at true
  // let c = []                   // LANG-2
  // let d = [1u64; 2]            // LANG-2
  // let e: Inline[u64, 3] = [1, 2] // LANG-7: wrong length
}
```

`[]` and `[e; N]` retain their context requirements. No later call, binding
annotation, destructuring pattern or allocation prohibition revises the selected
kind or element type. An inferred List has ordinary List ownership, locality,
allocation and cleanup obligations; `@noalloc` never changes it to Inline.
Swift likewise lets array context override its default literal collection
([CSGen.cpp:1580–1713](https://github.com/swiftlang/swift/blob/adf10a038cc1cf884e4a0e08964ef69ff890ba0a/lib/Sema/CSGen.cpp#L1580)).
Hirsel deliberately fixes T from the first element alone, without Swift's
whole-literal joins or default element type.

Without an outer expectation, the first valid, non-bottom branch of an `if`/`else`
or `match` supplies the expected type for branches to its right in source order.
Returning or invalid branches provide no type. Later text never types an earlier
literal or repairs an undetermined constructor. A later branch that conflicts with
this prefix-derived type is LANG-2 at that branch's conflicting expression.

```hirsel
fn names(c: bool) {
  let name = if c { Some("raw.bin") } else { None() } // Option[String]
  let next = match c { true => Some("next"), false => None() }
  // if c { Some("raw.bin") } else { Some(1u8) }    // LANG-2 at Some(1u8)
  // if c { None() } else { Some("raw.bin") }       // LANG-2 at the first None()
}
```

The branch context follows Rust's incremental `CoerceMany` idea
([coercion.rs:1511](https://github.com/rust-lang/rust/blob/1a2c4797ef2570bd76ca5dee34449169bc7dc457/compiler/rustc_hir_typeck/src/coercion.rs#L1511)):
keep an outer expectation or an already checked branch's type for subsequent arms.
Swift also joins branch results and treats `Never` specially
([CSGen.cpp:3507](https://github.com/swiftlang/swift/blob/bf3cf2cfb69ebb025b4ffce538fe3cde027d5246/lib/Sema/CSGen.cpp#L3507)).
Hirsel adopts no least-upper-bound widening, fresh join variables, earlier-arm
rechecking or whole-expression constraint solving: the prefix fixes one type. A call whose labels are invalid withholds only the contextual typing that
depends on them; independently typed operands are still checked.

Generic inference separates a root value's holder modes from a type contract supplied by
a part or an expected type. It does not infer root uniqueness or written sendability from
a value occurrence; it keeps the refinements a type-contract occurrence requires when the
generic parameter admits them. For a parameter with copying permission, inference may
forget a sendability requirement only where ordinary read-only variance permits that
weakening; it never weakens an invariant or mutable contract and never infers a forbidden
qualified actual from an expected result. An explicit qualifier on an occurrence is
checked at that occurrence and does not necessarily become a qualifier of the inferred
type argument. These decisions use only the declared signature and the argument and
expected-type constraints already available.

### LANG-3: Evaluation

Evaluation is strictly left to right: arguments, tuple and constructor fields, operands,
then the call. An argument moved by `take` remains a caller-owned temporary until the
callee is entered. On an early exit, each scope being exited drops its evaluated
temporaries in reverse evaluation order and then its named locals, innermost scope
first. A temporary that a `let` binding's value reaches is kept and released like a named
local declared just before that binding, on every exit.

A value is **live** at a point when some execution path from that point reaches a use of
it ([places and loans](ownership.md#places-and-loans)); a loan ends when no value holding
it is live, not at the end of a lexical scope.

Written `take` and `copy` operators evaluate inside out before their consumer:
`take copy x` extracts the prepared copy, and `copy take x` leaves `x` moved. Borrowing
the prepared value does not undo the extraction. A `mut` argument evaluates its address at
its source position and reserves the storage; its exclusive loan starts only at callee
entry. Later arguments may read it, but moving or writing it, a second exclusive
reservation, or a view still live at entry is OWN-3 at the call. A borrowed argument that
is not plain lends from its evaluation; a plain argument is a snapshot and lends nothing.

### LANG-4: Names

Names resolve against the program's definitions; there are no import statements. Module
path heads live in their own namespace and resolve only before a `.`. Each directly
selected dependency supplies its top-level module heads, with additional aliases from
its manifest `expose` entries. A head naming several distinct dependency targets is
LANG-4 at its use, even if only one target contains the member; the diagnostic names
the candidates. Writing the full package/version path disambiguates.

The exception is matching core and std modules: their identical module paths form one
head namespace, so `string.byte_len` names core and `string.find` names std. A definition name
present in both is LANG-4 when std is checked, even if unused. An unknown member under
a merged head lists the module paths searched. Fully versioned paths, including
`std@1/string.find`, remain legal; the version remains part of DefPath identity.

Variants are written bare (`Ok(x)`, `Usage`, `true`) and resolved through the expected
type. A local may not shadow another local or a top-level value declared in the same
module. It may shadow a top-level value from any other module, including the package
root. Unqualified value lookup checks locals, then the current module, then the package
root; the root fallback applies only when no local or current-module value matches.
Same-package lookup precedes dependency lookup.

Core library roles such as `Unsafe`, `Result` and the capability types are identified by
fully qualified definition paths, never by spelling alone; a user type with the same name
is a distinct type and cannot fill that role.

Core names are defaults. A bare name means a core definition only when nothing else
answers it in that position: a local, a same-package definition, or a contextual variant
selected by the expected type, including a dependency type's variant. This holds in every
position: type declarations and annotations, constructors and other values, patterns,
`?` mappers ([LANG-8](#lang-8-errors)) and attribute arguments. The shadowed core
definition stays reachable by its full path, as in `core@1/Map[K, V]`. Builtin semantics
attach to the core path, never to the spelling, so a user `Map` is an ordinary nominal
type that core's `map` operations do not accept. Where a diagnostic shows two types with
the same name, it spells both by their full paths, as in `app@1/Map[u64]` and
`core@1/Map[K, V]`. Primitive names and the constructors with no core declaration
(`FnPtr`, `TypeShape`) are reserved instead ([LANG-7](#lang-7-types)).

A contextual variant selected by the expected type wins over a same-named function
or type. This applies through nested constructor arguments and patterns, whose
expected type is the payload's declared type with the enclosing type's arguments
substituted: with expected `Option[V]`, `Some(Array(xs))` selects `V`'s `Array`
variant when `V` declares it, and matching a `Result[T, http.HttpError]` against
`Error(Net(_))` selects `HttpError`'s `Net`, not core's `Net` capability type.
Without an expected variant type, `Net` names core's type only when no same-package or
dependency type in view declares a `Net` variant; otherwise it is a bare variant without
an expected type (LANG-2). A
versioned path consumes name and slash separators greedily up to its first dot,
independently of whitespace; a member or numeric right operand ends the path for division.
To divide a versioned path by a name, group the path: `(pkg@1/a) / b`. Parsing never
consults a manifest.

### LANG-5: Mutation

There are no references. A value changes only through a visible name: a local `let mut`,
a `mut` parameter passed as `mut x`, or a `for x in mut xs` element place. Capabilities
and handles name external things: copying one does not copy what it names.

```hirsel
fn total(xs: List[u64]) -> u64 {
  let mut sum: u64 = 0
  for x in xs { sum += x }
  sum
}

fn bump(mut xs: List[u64]) {
  for x in mut xs { x += 1 }
}
```

### LANG-6: Signatures

A signature is mandatory on every top-level definition: types, parameter modes and value
modes, relations, brands, capabilities, allocation contract, contracts and error type. No
part of a signature is ever inferred from a body, and the calling convention follows only
from what the signature declares. Inference happens only inside bodies.

Static checking of a definition uses its body and only the declared signatures of the
other definitions it uses. Which implementation is selected for each definition
determines execution ([PROG-1](programs.md#prog-1-names)). A body edit that keeps its
signature rechecks only that definition; [GEN-5](generics.md#gen-5-polymorphic-recursion)
is the one separate program-level check.

"Mandatory" means determined by signature text, including fixed defaults:

- an omitted result is `()`;
- allocation is permitted unless `@noalloc` is written;
- modes follow [MODE-7](ownership.md#mode-7-inference): borrowed and `mut` parameters are
  local to themselves, taken parameters and results are global, values are aliased,
  closures are many;
- relations, brands and contracts are absent unless written;
- authority comes from the declared capability-bearing parameters, and a closure's
  authority is unrestricted unless `uses` restricts it;
- the error type is the `E` of a declared `Result[T, E]`; otherwise there is no
  expected-error channel.

A definition containing `todo` is marked `@incomplete` in its signature, and so is one that
calls an incomplete signature; complete definitions cannot call it. Bodyless declarations
are signature fragments, and execution requires an implementation. Inference of a closure
literal's type happens inside its enclosing body and never becomes an exported signature.

The zero-argument `@track_caller` attribute applies to ordinary named function items,
including generic functions and signature fragments. It is invalid on extern declarations,
closure literals, local closure declarations and callable types. It declares failure
attribution ([FAIL-1](failure.md#fail-1-kinds)); it is declaration metadata and does not
change the source callable type, allocation contract or authority.

A known named function value carries `uses` evidence derived only from its declared
signature; an unannotated closure literal derives its evidence from its captures and its
header. Every written callable type keeps its contract, including an unrestricted `uses`.
Materializing compile-time values depends on the executed implementations and constants,
so a same-signature body edit invalidates evaluated dependents, while static checking
still reads only signatures.

**Definition hashes.** Every definition has a definition hash, the code identity that
runners, the code store and upgrades use. It covers what the definition means, never what
the definition or anything it uses is called:

- A definition with a body hashes its signature and its TIR; a type hashes its owner view,
  with its finalizer's TIR when it has one. Every other definition the encoding names is
  written as that definition's hash, except the nominal roles every signature carries
  (the core definitions that are the `Result`, `Unsafe`, `Tasks`, `Scope` and heap
  `Arena` roles), which are written by role name: a definition that uses a role type names
  it in its signature or TIR, so an edit to a core role type moves only the definitions
  that use it. Source positions, the names of locals, parameters and
  generic parameters, and the definition's own name are erased. Labels and field and
  variant names belong to the signature and remain.
- A definition whose meaning is attached to its name hashes its full versioned path and its
  signature: a registry operation, an extern or a signature fragment. A generic generated
  declaration ([CT-5](compile-time.md#ct-5-generation)) also hashes its path, its
  signature, its declaration's tokens and the hashes of what its generator reads, so its
  hash changes when it is renamed.
- A recursive group (a strongly connected component of the reference graph) is hashed
  together. A reference into the group is written as a group marker. Each member's key is
  a rooted depth-first encoding of the group from that member: each member's text, then
  its group references in encoding order, each written as the discovery ordinal of a
  member already reached or as that member's encoding when first reached. Its hash is that
  key under the definition-hash domain.

Renaming a definition, local or module keeps every hash except those of the name-bound
definitions listed above and their dependents. A body or signature
edit changes the edited definition's hash and the hash of every definition that reaches it,
and two identical definitions in different modules share a hash. A definition hash is not
a type identity for checking or serialization: nominal identity stays with paths and
signature fingerprints, which a caller checks alongside it. The encoding is the
[PS-8](proof-seam.md#ps-8-encoding) wire text with these substitutions, hashed with the
length-framed SHA-256 of [PS-5](proof-seam.md#ps-5-tir-digest) under the domain
`hirsel.definition.hash.v2` with `TIR_VERSION`. For the same definitions it is the same
on every machine and build, whether a package was checked from source or adopted from its
precheck, and changes only with `TIR_VERSION` or the encoding. `hirsel hash` prints the hashes of a
program's definitions, and the definition-hash query lists each call site with its
callee's hash.

Whether a generic parameter can transport an implicit sendability refinement follows
from its declared copying permissions, never from its implementation. Neither a body that
happens not to copy nor a library operation's name strengthens that contract. Core
signatures obey the same rule.

### LANG-7: Types

A user type name cannot be a primitive type name (`bool`, `char`, the sized integers and
floats, `String`, `Bytes`, `Never`) or `FnPtr`, `TypeShape` or `any`; choose a domain
name. Literals and operators produce the primitives, and `FnPtr` and `TypeShape` have no
core declaration a full path could reach. Every other core type name, including
`Array`, `List` and `Map`, may be declared and shadows the core one
([LANG-4](#lang-4-names)).

Types are algebraic data types with exhaustive `match`, generics with structural bounds
and explicit dictionaries ([generics](generics.md)), size parameters `[n: size]` (equality
only), and `distinct` types, which inherit their base's bounds, are built as `Percent(v)`
and read back with `let Percent(v) = p`. There is no overloading, no type class, no
implicit conversion (between error types only [LANG-8](#lang-8-errors)'s `?` arm injection), no textual macro expansion, no
exception, no `any` and no cast. Function bodies can be generated at compile time for headers written in source
([CT-5](compile-time.md#ct-5-generation)).

The forms are `bool`, `char`, sized integers and floats, `String`, `Bytes`, aliases `type A = B`,
single variants `type A = Only()`, records, variants, tuples (`(x,)` for one element),
`box e`, sequence literals, and match arms with guards `Pattern if condition =>
expression`; guards are pure and do not establish exhaustiveness. Bounds join with `+`;
opaque bounds and authority are written explicitly.

`char` is a Unicode scalar value: an integer in 0 to 0x10FFFF other than a surrogate
(0xD800 to 0xDFFF), stored in 4 bytes, and plain, sendable and a key like the integers.
It is not an integer. `==` and `!=` compare chars, and `<`, `<=`, `>` and `>=` order them
by scalar value; every other operator on a `char`, a `u32` where a `char` is expected and
a `char` where a `u32` is expected are LANG-7. `char.to_u32` and `char.from_u32`
([core library](core-library.md#numeric-and-string-details)) convert explicitly, the
latter returning `None` for a value that is not a scalar value.

```hirsel
fn later(a: char, b: char) -> bool { a > b }              // a + b is LANG-7
fn digit(c: char) -> bool { c >= '0' && c <= '9' }
fn code(c: char) -> u32 { char.to_u32(c) }                 // char.to_u32('a') is 97
fn scalar(n: u32) -> Option[char] { char.from_u32(n) }     // `n` alone is LANG-7
```

The modifiers `unique`, `once` and `sendable` precede the type they qualify and cover all
of it, a function type's result included. A brand `of p` and a relation `in S` follow the
type, in that order, and bind to the nearest type on their left; a function type ends at
its `uses` clause. So `fn() -> T in x` relates the result, `fn() -> T uses {} in x`
relates the function value, and `(fn() -> T) in x` relates a function value whose type
has no `uses` clause. Relations are written on whole values only: a record field, variant
payload, tuple element, `box` payload or type argument (a part type) carries none, even
through an alias or grouping (LANG-7 at that type; [OWN-8](ownership.md#own-8-parts)). The
qualifiers `@ unique`, `@ once`, `@ sendable`, `@ local` and `@ global` do not exist;
writing one, or repeating a modifier, is LANG-7 at the type. Allocation attributes precede
declarations or function types.

`sendable` may qualify any type, through grouping and transparent aliases, including a
type parameter and a part type. A qualifier before a function type qualifies the function
value; a qualifier in its result qualifies the returned value. It does not distribute into
the function's parameters.

Unlabelled function-type parameter names are binders, not part of type identity; labels
and passing modes are significant. Each nested type, size and value binder is distinct,
even at the same position. A relation or `unique` on a plain type is inert; identity
brands and capability authority are never erased. A resolved scalar, tuple or variant
type name used as a value is LANG-7 at the name; aliases follow their representation. A
bare nonempty record type name instead needs labelled construction
([LANG-12](#lang-12-arguments)), and a generic record needs its expected instantiation
([LANG-2](#lang-2-prefix-law)). A duplicate top-level definition path is LANG-7 at the
later name. Type incompatibility with no narrower owner is LANG-7.

`Never` is the type with no values. A call whose instantiated result is `Never` diverges:
it never completes normally, so it fits any expected type in any value position (a match
arm, a `let` initializer, an argument), and its only exit is a language failure, which
cleans up the values already evaluated as any failure does. A body declared `-> Never` must
itself have type `Never`, so it ends in such a call; any other value is LANG-7. A self tail
call of a `Never` function is a loop like any other self tail call
([LANG-10](#lang-10-recursion)).

A repeat literal `[e; N]` requires an expected `Array[T]` or `Inline[T, n]`.
The context types `e` as `T` and `N` as `u64`. `N` is evaluated at definition
time, through the ordinary compile-time evaluator; runtime values and abstract
size parameters are CT-2. The resulting Array has length `N`; an Inline requires
`N == n`, otherwise LANG-7. List is not a repeat-literal target (LANG-7).

`e` is evaluated exactly once, even when `N == 0`, and its value is copied into
each element. A nonconstant `e` must be copyable (OWN-5, or GEN-1 for an
unbounded generic parameter); constant expressions are also admitted. The
ordinary copy and cleanup rules apply. For example:

```hirsel
fn repeated_values() {
  let raw: Inline[u8, 64] = [0u8; 64]
  let words: Array[String] = ["hello"; 3]
}
```

### LANG-8: Errors

Expected failures use `Result` and `?`. `e ? C` maps the error through `C`, a path naming
a one-argument variant or function, before propagating. A type is never a mapper: when a
bare `C` also names a type, such as core's `Net`, `C` is the enclosing error type's variant
([LANG-4](#lang-4-names)). An error mapper may borrow or take its owned error; a mapper that
requires `mut` access is LANG-8.
The required mapper contract is `fn(SourceError) -> EnclosingError`. A nullary arm
does not discard the source error. To do so intentionally, declare a named unary
helper, for example `fn invalid_json(error: json.JsonError) -> CommandError uses {}
{ InvalidJson() }`, and write `json.stringify(candidate) ? invalid_json`. The form
after `?` is a path, so a closure literal cannot be substituted there.

A plain `e?` propagates an error identical to the enclosing function's or closure literal's
error type unchanged. Otherwise it injects the error through an arm of the enclosing error
type: the one arm of its variant with exactly one field whose type is identical to the
error. Only a nominal error is injected: a named type, generic applications included, or a
type parameter. A primitive (including `String`, `Bytes` and `Never`), tuple, function or
builtin error is not, and needs `? C`. It is LANG-8 when no arm carries the error, when two
or more do, or when the enclosing error type is not a variant visible here (a type
parameter, or an opaque type of another package). Injection is one level deep: an arm
whose payload could itself carry the error does not. The arm is selected once, when the
definition is checked against its declared types; instantiating a generic definition never
re-selects it, so in `Mixed[E] = Source(E) | Files(FileError)` a generic body injects an `E`
through `Source` even where `E` is later `FileError`. A plain `?` and `? Arm` naming the arm
it selects mean the same thing and produce the same definition hash.

When `e?` has expected type `T` in a Result-returning function, `e` is checked with
expected type `Result[T, F]` for a fresh `F`, then the existing error rules apply.

```hirsel
type ParseError = Empty | TooLong(u64)
type AppError = Input(ParseError) | Other

fn parse(text: String) -> Result[u64, ParseError] {
  if string.byte_len(text) == 0 { return Error(Empty) }
  Ok(string.byte_len(text))
}

fn run(text: String) -> Result[u64, AppError] {
  let n = parse(text)?             // injects ParseError through Input
  Ok(n + 1)
}
```

Two arms carrying one type are two roles, so plain `?` names neither:

```hirsel
type DbError = Open(store.StoreError) | Commit(store.StoreError)

fn save(dir: Dir, take data: Bytes) -> Result[(), DbError] {
  let mut s = store.open(dir, store.default_config()) ? Open   // plain ? is LANG-8 here
  let mut t = store.begin(mut s)
  txn.put(mut t, b"blob", take data)
  store.commit(mut s, take t) ? Commit
  Ok(())
}
```

### LANG-9: Tests

Tests are ordinary functions, pure or taking the `Test` capability, which the test runner
selects and calls. The zero-argument `@test` attribute selects an ordinary, complete,
monomorphic Hirsel function declared in the manifest's own sources. It has no parameters
or one borrowed `Test` parameter and returns `()` or `Result[(), E]`; visibility does not
affect selection. Other signatures are LANG-9. Tests remain callable as ordinary functions.
Each selected test runs in isolation with only Test authority: checks, an Error result,
and runtime failures fail that test while ordinary cleanup runs and later tests continue.
The default seed is the first eight SHA-256 bytes of the UTF-8 stable definition path,
interpreted little endian; an explicit runner seed overrides it. Assertions are explicit
(`test.check(t, cond)`); property tests are a library call over an explicit generator
(`test.property(t, gen, law, show)`) whose seed comes from `Test` and whose failures shrink
to a minimal counterexample. Fakes, such as a confined directory or a scripted network, are
created from `Test`. `test.tasks(t)` supplies deterministic simulated tasks under the run's
scheduler; `test.clock(t)` supplies a fake monotonic clock that advances only through
simulation. A blocked or deadlocked test fails and reports its outstanding waits.
A false check fails LANG-9. The zero-argument `@bench` attribute
selects a benchmark under the same signature rules; the runner calls it only when asked for
benches, and a function carrying both `@test` and `@bench` is LANG-9.

```hirsel
@test
fn test_sum(t: Test) {
  test.check(t, 2u64 + 2 == 4)
}
```

### LANG-10: Recursion

Only top-level definitions recurse; a local `fn` cannot refer to itself or to later
locals. Self tail calls are guaranteed loops. Where a self tail call passes an owned
temporary to a borrowed parameter, the loop keeps it in an internal owned slot (one count
operation per call, not per iteration), released on replacement and on final exit. A self
call whose argument borrows the current frame is not a tail call, and neither is one with
pending user finalization, ordinary release, loan cleanup or postconditions after it. A
checked self tail site reuses the frame and keeps its lexical size bindings and the
caller's continuation.

```hirsel
fn sum_to(n: u64, acc: u64) -> u64 {
  if n == 0 { return acc }
  sum_to(n - 1, acc + n)      // a loop, however large n is
}
```

Stack use is a logical count of frame bytes with a deterministic limit, whose failure is
`Depth`. **Unspecified:** the logical frame-byte table, and whether tail recursion modulo
constructor is a guaranteed loop (its constructor-tail contexts are reserved and
rejected). Runtime call-depth guards in tools are operational limits, not this contract.

### LANG-11: Blocks

A block's value is its final expression. `return` exits the enclosing `fn` (a closure's
own included) from any position, and is rejected only as the final statement of a `fn` or
closure body, where the plain final expression says the same thing.

A block, branch or arm is transparent to its consumer's use. A tail that names an outer
place keeps that place's identity and field partition; a binding declared inside moves
out to the consumer, even when the consumer only borrows. At a borrowed join, an owned
alternative lends a temporary in the consumer's scope. A computed borrow of outer storage
is not a source place, so `mut` or `take` of it fails OWN-3. A plain projected computed
part is a copy and may be mutated as a temporary; a counted projected part stays
borrowed.

```hirsel
fn clamp(x: u64) -> u64 {
  if x > 10 { return 10 }
  x
}
```

A labelled block `'l: { .. }` is an expression whose value is its tail or the operand of a
`break 'l value` inside it, which leaves every enclosing scope up to and including the
block's (`break 'l` alone gives `()`). The tail and the break operands join as an `if`'s
branches do: they have one type, checked against the block's expected type or else taken
from the first of them, and the block is transparent to its consumer's use through each of
them. Only a labelled block takes a break value: a loop has no result, so wrap the loop in
a labelled block to compute one. `break 'l value` has type `Never`, evaluates its operand
first, then ends the loans and cleans up the scopes it leaves exactly as a labelled loop
exit does. A labelled block's label shares the loop-label namespace and its rules. An
unlabelled `break` or `continue` cannot leave a labelled block to reach a loop outside it,
and `continue` cannot name a block (LANG-7).

```hirsel
fn find(rows: List[List[u64]]) -> Option[(u64, u64)] {
  'search: {
    for i in 0u64..list.len(rows) {
      for j in 0u64..list.len(rows[i]) {
        if rows[i][j] == 7 { break 'search Some((i, j)) }
      }
    }
    None()
  }
}
```

### LANG-12: Arguments

A parameter declared with a label (`fn rename(from src: Path, to dst: Path)`) must be
labelled at every call; an unlabelled parameter cannot be. Labels precede modes:
`f(into: mut xs)`. Records are built with every field labelled; variants declared with
positional payloads (`Ok(x)`) and tuples are built positionally. Labelled arguments may
appear in any order, each required label once. Evaluation follows the written order, and
the callee receives them in declaration order.

```hirsel
fn span(from start: u32, to end: u32) -> u32 { end - start }
fn caller() -> u32 { span(to: 9, from: 2) }
```

### LANG-13: Patterns

A structural or literal pattern on `box T` inspects `T` through an implicit payload
projection, recursively through nested boxes. A binding or `_` keeps or ignores the whole
value; a contextual nullary variant name is a structural pattern. `let` accepts only
irrefutable patterns (tuples, records, `distinct` types, `_`), unless it has an else
block. Other refutable patterns appear only in `match`, `if let` and `while let`.

`let P = e else { .. }` binds `P` in the enclosing block when `e` matches, and otherwise
runs the else block, which must diverge: its type is `Never`, through `return`,
`break`, `continue` or a call that never returns. `P` must be refutable, and the
bindings are not in scope in the else block. The pattern is checked as a match arm is:
an unmarked binding place is inspected in place and its parts borrowed, while `take e`
or a computed value is taken, so its bindings borrow or take exactly as `match` binds
them. The scrutinee and the scopes the else block leaves release under the ordinary
cleanup rules. The initializer cannot end in a block (`if`, `match`, a block or a
closure literal) unless it is parenthesized: `let Some(x) = (if c { a } else { b }) else
{ .. }`.

A conditional let is a match with one refutable arm. `if let P = e { A } else { B }` is
`match e { P => A, _ => B }`, and without an else it is `match e { P => A, _ => {} }`, so
`A` has unit type as in an `if` without else. `'l: while let P = e { A }` is
`'l: while true { match e { P => A, _ => break } }`: `e` is evaluated before every
iteration, and `break` and `continue`, labelled or not, leave or repeat the loop. The
bindings of `P` are in scope only in `A`, and they borrow or take exactly as the match
arm's do. `P` must be refutable. There are no let chains: the scrutinee cannot be an
unparenthesized `&&` or `||`; nest an `if` instead.

```hirsel
type Figure = Circle(radius: u32) | Rect(w: u32, h: u32) | Empty

fn area(s: Figure) -> u32 {
  match s {
    Circle(radius: r) => 3 * r * r
    Rect(w: w, h: h) if w == h => w * w
    Rect(w: w, h: h) => w * h
    Empty => 0
  }
}
```

A range pattern `a..=b` matches the integer or char scalars from `a` to `b` inclusive.
Both bounds are literals of the scrutinee's type (integer, character or byte literals,
a negative integer written with its sign); a range whose start exceeds its end is
rejected, and there is no half-open range pattern. Ranges and literals partition a
scalar's domain for exhaustiveness, so arms covering every `u8` or every Unicode scalar
need no `_`. Arms are tried in order and overlapping ranges are allowed, but a range
pattern no value reaches, because earlier unguarded arms match every value it
matches, is rejected.

An or-pattern `P | Q` matches a value either alternative matches, trying them left to
right, anywhere a pattern is allowed, nested too. Every alternative binds the same names,
each with the same type and modes; alternatives normalize into the exhaustiveness
judgment, so `let Ok(v) | Error(v) = r` is irrefutable. An alternative no value reaches,
because earlier arms or earlier alternatives match every value it matches, is rejected,
and a pattern may match in at most 256 ways through its alternatives. A match arm's
bindings follow `match` exactly, borrowed or taken, and so do those of `if let` and
`while let`. In `let`, let-else and `for`, the matching alternative moves or copies each
binding into the one binding its name declares, so their or-patterns bind plain values or
take their payloads: a binding that would borrow is rejected. A borrowed binding is a view
of one place of its source, a fixed path of fields and variant payloads, and alternatives
bind a name at different places (`A(x) | B(x)` views `t`'s `A` payload or its `B`
payload). A match arm's block is the scope of one way to match, so each view stays one
place; a `let` binding's scope continues past the choice, where one view would stand for
a place chosen at run time, and there are no references (LANG-5). Bind such a view with
`if let` or `match`, or take the value.

Destructuring a value of a type that declares `drop` is allowed only in that type's
defining module. A committed whole destructure consumes the resource without running its
finalizer: bound fields move to their bindings, and fields omitted with `_` or a rest
pattern are dropped normally, in declaration order, at the destructure, on every exit
path including failure. This does not permit a partial take such as `take r.f`.

### LANG-15: Bindings

`let y = x` copies a `plain` value and moves any other; `let y = copy x` shares instead.
Binding a borrowed parameter gives another borrow with the same limits; binding a `mut`
parameter gives a read-only borrow that ends at the binding's last use. A closure literal
bound by `let` is scoped unless its binding has an escaping type; a closure value returned
by a call keeps its type.

A body-local `let mut` whose initializer is a read-only borrow of a non-plain type
satisfying `data` and `copyable` is a **borrow cursor**. Its access remains read-only,
but simple assignment to its whole name may replace the view it holds. Initializers
otherwise keep the rules above: a relation annotation does not change a move, copy,
construction or call result into a borrow. Parameters themselves are not borrow cursors.

A cursor's receiving contract fixes its admissible origins at its declaration. With no
written relation these are the initializer's underlying value places and inherited
bounds, after following read-only aliases. A written relation may instead name value
places already established in an enclosing scope; it cannot name the cursor itself or
consist of `heap`, `env`, an arena or a scope. Each named place admits its subplaces and
the bounds supplied by its declared relation. Inherited arena and region bounds remain
in force. Later assignments never enlarge this contract.

In `cursor = expression`, the expression is evaluated as a read-only view, with block,
branch and match tails transparent to that use. Each possible view must be within the
cursor's fixed origin contract and every storage origin it requires must remain live for
all uses of that view. Owned expressions, including `copy`, `take`, constructions and
call results, cannot replace a cursor. An origin outside the fixed contract is MODE-1,
or PROV-4 under that rule's arena-only diagnostic condition; a non-view replacement or
an unsupported cursor relation is LANG-15. An immutable name is still LANG-5 on
whole-name reassignment.

Rebinding evaluates and checks the new view before replacing the old one. It writes only
the cursor binding, does not mutate, move, copy or destroy the referent, and performs no
count operation. A failure before replacement leaves the old view installed. Projected
writes, compound assignments and `mut` or `take` uses gain no permission from `let mut`
on a cursor. Ordinary aliases and physical data-pattern bindings retain the view they
received, independently of later cursor rebindings; borrows of the cursor's binding
storage itself still conflict with a rebind.

At a control-flow join the cursor may hold any view carried by an incoming path. Its
current origin and loan sets are the union of those paths; its receiving contract is
unchanged. Loops include their entry and all feedback paths in this check. Each
execution of a declaration creates its own origins, and a live view cannot survive the
end of any origin it reaches.

For example, `fn size(a: List[u64]) -> u64 { let mut c = a; c = a; list.len(c) }`
rebinds a borrowed view. In that body, `c = copy a` is LANG-15 because the replacement
is owned; `c[0] = 1` is LANG-15 because the view grants no referent write. Adding an
unrelated borrowed parameter `b` does not admit `c = b` without a written `in {a, b}`
cover. An owned initializer such as `let mut c = copy a` keeps ordinary owned assignment.

A binding's **receiving contract** is fixed at its declaration: a written relation
(`let x: T in S`, [MODE-1](ownership.md#mode-1-locality)) supplies it, and otherwise the
initializer does. A binding named in its own relation stands for its own scope, so
`let mut r: Holder in r` receives any content that outlives `r`. An unmarked annotation
forgets extra brands of the initializer. Ordinary mutable root storage is aliased even
when its current content is freshly unique; explicitly `unique` storage keeps that
requirement. A binding's current content is its loan set at each point, a flow fact
separate from its contract. Contracts stay fixed for body-local storage as for
caller-owned storage. Owned local storage keeps its private lifetime after a global
refill, and projected writes can store new content bounded by its sources within that
lifetime. Caller-owned borrowed or `mut` storage cannot receive content that would die
with this frame.

A written `sendable` fixes a binding's crossing requirement on initialization and on each
replacement. An annotation without `sendable` adds no such requirement, and a derived
sendability fact alone never turns mutable data storage into a permanently sendable
receiving contract. Current evidence may weaken through ordinary ownership flow, and a
declared requirement cannot restore it without a checked receipt. Written callable and
erased-state types keep their declared environment promises rather than the stronger
summary of their initializer.
