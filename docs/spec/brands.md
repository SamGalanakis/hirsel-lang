# Brands and placement

A **brand** `of p` says a value belongs to one particular value `p`; a handle branded by a
directory can be used only with that directory. **Placement** says where a value's blocks
live: in an explicit arena, or wherever [regions](memory.md#mem-10-regions) put them.
Both are loans ([places and loans](ownership.md#places-and-loans)): a brand is an identity
loan and arena placement is a bound loan.

### PROV-1: Brands

`x: T of p` says `x` belongs to the particular value `p`: a borrowed parameter, a local
binding, a field path (`log.dir`), a top-level definition (identity by name) or a sibling
field (`File of .dir`). An operation taking a branded value requires the same `p`.

```hirsel
fn size_of(d: Dir, f: File of d) -> u64 { 0 }

fn caller(a: Dir, b: Dir, p: Path) -> Result[u64, FileError] {
  let f = file.open(a, p)?       // File of a
  Ok(size_of(a, f))              // size_of(b, f) is PROV-1
}
```

A brand ends when `p` is reassigned, moved, mutated or leaves scope, except that container
brands (`SlotId of s`) follow the container's identity
([MEM-14](memory.md#mem-14-slots)). A copy of `p` keeps `p`'s identity only until the
copy's first mutation or reassignment. Brands never name `take` parameters.

A value whose brand has ended may still be dropped, or moved whole into body-owned
storage while carrying the expiry; its first later read, call, return, part store or `mut`
hand-back is PROV-1. Temporary identities end with their statement unless held, and
block-local identities with their scope. `take` consumes an identity before later
arguments. Unbranded borrowed and taken parameters may forget the brands of data graphs;
`mut` and non-data parameters cannot forget nested brands
([PROV-2](#prov-2-checking)). Mutating a subplace ends the brands of the containing owners
and of descendants, not of unrelated siblings; an indexed mutation renews the container,
not an element's identity. A whole owned replacement gives a fresh identity. Joins on a
continuing path exclude exiting arms, each loop iteration introduces a fresh incarnation,
and mutating a copied container preserves the detached original's owners.

### PROV-2: Checking

Unmarked results carry no brand. At a call the checker substitutes argument names for
parameter names. At branch joins brands take the union, and a value whose brand is a union
is a type error at its first branded use, except for handles, which
`file.narrow(d, f) -> Option[File of d]` narrows at run time. A closure's result may not be
`of` the closure's own parameters, and its relation may not name an arena parameter of the
closure. There are no lifetime variables.

Arithmetic, logical and bitwise computation, unary operations included, produces
unbranded results. An expected annotation cannot certify a brand. A comparison sets aside
root brands, locality and uniqueness while preserving nested types and placement, and
returns an unbranded `bool`. A comparison only borrows, so an operand of every comparison
operator (`==`, `!=`, `<`, `<=`, `>`, `>=`) is checked against the type it inherits from the
other side with every `unique` qualifier, nested ones included, removed:
`base64.decode(x) == Ok(data)` accepts a borrowed `data: Bytes`, and a `unique String`
orders against a borrowed one. `copy` and `take` transport existing evidence; a freshly
computed owner has a fresh identity. A sampled opaque value's brand (such as an `Instant`
of a clock) is introduced only by its declared primitive producer and preserved by
ordinary substitution and payload transport; a written expected brand or a decoder cannot
create it.

### PROV-3: Uses

The core library brands: `File of dir`; `StrPos of s`; `SortedMap[K, V] of by` (combining
two maps requires the same `by`); `Task[R] of s`; `SlotId[T] of slots`; `Instant of clock`
for a `MonotonicClock`; `Stream of n` and `Listener of n` for a `Net`.

### PROV-4: Arena placement

A relation that names an arena (`x: T in a`) says `x`'s blocks live in `a` or in something
that outlives it; `heap`, the empty relation, outlives every arena. A value in `a` cannot
outlive `a`. An explicit arena is a conservative signature restriction, legal even for
read-only, nonallocating functions; no body inspection decides whether it is necessary. A
typical use is `push[T](a: Arena, mut xs: unique List[T] in a, take x: T in a)`.

A relation names at most one arena directly. Constructors, among them `map.empty()` and
`set.empty()`, building a value whose relation names an arena place fresh blocks there;
otherwise they are placed by the expected type, else as [MEM-10](memory.md#mem-10-regions)
chooses. A relation that names parameters places fresh blocks where they agree: one arena
reached by every named source is the destination, global sources add no bound, and distinct
arenas or local sources without an arena leave the destination to MEM-10. A branch or match
join holds the union of its arms' loan sets at every payload depth, so joining arena
content with global content stays bounded by the arena. A constructed value carries its
parts' arena placement. An argument holding only arena content bounds a call result by its
arena alone.

```hirsel
type Tree = Leaf | Node(left: box Tree, key: u64, right: box Tree)

fn build(a: Arena, depth: u64) -> Tree in a {
  if depth == 0 { return Leaf }
  Node(left: box build(a, depth - 1), key: depth, right: box build(a, depth - 1))
}
```

A function returning `build(a, 3)` without `in a` is PROV-4: an arena value cannot be kept
where no placement is declared.

**Classification.** When a value is not within a required relation (a result, storage, a
taken argument, or a capture by an escaping closure, which stays
[CLO-4](closures.md#clo-4-escaping)), the failure is PROV-4 if every bound loan it holds
outside the relation is on an arena, and [MODE-1](ownership.md#mode-1-locality) if any is
on a binding, field path, own root, body region or `env`. So an arena value returned or
stored globally is PROV-4, and a borrowed view of an arena value is MODE-1, since its own
root also escapes. An arena- or region-placed value kept where no placement is declared is
PROV-4, also when it is local.

A relation cannot name a taken arena parameter in any signature position (PROV-4 at the
relation), except a taken parameter naming itself (`take a: Arena in a`). A global
requirement (a result, storage, part or nongeneric taken parameter without a relation)
cannot receive arena content; borrowed parameters may read it. A generic actual carries
its relation in `T`. `env` names no placement source. `in heap` is the empty relation: any
global value meets it, since a global value cannot hold arena content. Placement of plain
values is inert. A `let` relation naming bindings means "within `S`" in this check, and a
relation naming a field path `z.p` admits only what `z.p` holds.

### PROV-5: Sibling brands

A constructor argument for a field typed `of .f` must be of the expression supplied for
`f` in the same constructor. Moving the record keeps the relation; destructuring it
rebrands the field to the new local bound for `f`. A field named by a sibling's brand is
neither replaced nor passed `mut` except together with the whole record.

```hirsel
resource type Log { dir: Dir, file: File of .dir }

fn open_log(d: Dir, p: Path) -> Result[Log, FileError] {
  let f = file.open(d, p)?
  Ok(Log(dir: d, file: f))       // file is of the dir supplied beside it
}
```
