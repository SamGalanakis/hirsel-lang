# Language

This part covers how expressions are typed and evaluated, how names resolve, what a
signature fixes, and the forms of types, errors, tests, recursion, blocks, arguments,
patterns and bindings. The grammar is in [Syntax](syntax.md); locality, uniqueness and
ownership are in [Ownership and modes](ownership.md).

### LANG-2: Prefix law

Every type judgment is decided by the text to its left plus the expected type. Type
arguments are never written in expressions, so a postfix `[` is always indexing.
`shape(T)` is reserved syntax for future type-shape reflection
([CT-3](compile-time.md#ct-3-shapes)), not an executable API.

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

A top-level or imported generic function name may be used as a value when its immediate,
complete expected monomorphic callable type uniquely determines every type and size
argument from the declared signature. This includes annotated bindings, arguments, record
fields and returns. No later statement or implementation body participates. Without that context,
or with an undetermined argument, report LANG-2 at the name and name the undetermined
parameter. Lexical generic functions remain direct-callee-only. The instantiated callable
retains its declared modes, `uses`, allocation and relation contracts
([GEN-1](generics.md#gen-1-bounds)); ordinary callable receipt checks still apply. An
error-mapper path under [LANG-8](#lang-8-errors) is instantiated from the error it maps.
Without an expectation, the first valid, non-bottom branch of a conditional or match
supplies the context for later branches in source order; later text never types an
earlier literal. A call whose labels are invalid withholds only the contextual typing that
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

A contextual variant selected by the expected type wins over a same-named function.
This applies through nested constructor arguments: with expected `Option[V]`,
`Some(Array(xs))` selects `V`'s `Array` variant when `V` declares it. A
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

A known named function value carries `uses` evidence derived only from its declared
signature; an unannotated closure literal derives its evidence from its captures and its
header. Every written callable type keeps its contract, including an unrestricted `uses`.
Materializing compile-time values depends on the executed implementations and constants,
so a same-signature body edit invalidates evaluated dependents, while static checking
still reads only signatures.

Whether a generic parameter can transport an implicit sendability refinement follows
from its declared copying permissions, never from its implementation. Neither a body that
happens not to copy nor a library operation's name strengthens that contract. Core
signatures obey the same rule.

### LANG-7: Types

A user type name cannot be a primitive type name or a compiler-native
type constructor name; choose a domain name.

Types are algebraic data types with exhaustive `match`, generics with structural bounds
and explicit dictionaries ([generics](generics.md)), size parameters `[n: size]` (equality
only), and `distinct` types, which inherit their base's bounds, are built as `Percent(v)`
and read back with `let Percent(v) = p`. There is no overloading, no type class, no
implicit conversion (including between error types), no textual macro expansion, no
exception, no `any` and no cast. Typed compile-time generation is reserved
([CT-3](compile-time.md#ct-3-shapes)).

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

### LANG-8: Errors

Expected failures use `Result` and `?`. `e ? C` maps the error through `C`, a path naming
a one-argument variant or function, before propagating; a plain `e?` requires identical
error types. An error mapper may borrow or take its owned error; a mapper that requires
`mut` access is LANG-8.

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
  let n = parse(text) ? Input      // maps ParseError into AppError
  Ok(n + 1)
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
(`test.check(t, cond)`); property tests are a library call over an explicit generator (`test.for_all(t, gen, law)`) whose seed comes
from `Test`. Fakes, such as a confined directory or a scripted network, are created from
`Test`. A false check fails LANG-9.

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
bindings follow `match` exactly, borrowed or taken. Outside `match`, the matching
alternative moves or copies each binding into the one binding its name declares, so a
`let` or `for` or-pattern binds plain values or takes its payloads: a binding that would
borrow is rejected.

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
