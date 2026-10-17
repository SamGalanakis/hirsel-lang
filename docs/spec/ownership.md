# Ownership and modes

Hirsel has no references. Values are passed borrowed, `mut` or taken, and the checker
tracks four modes of every value: **locality** (global or local, and to what),
**uniqueness**, **multiplicity** (once or many) and **sendability**. Signatures declare
modes; bodies infer them. One model, places and loans, gives every locality judgment in
this part and in [brands and placement](brands.md), [closures](closures.md) and
[regions](memory.md#mem-10-regions).

## Places and loans

A **place** is a binding (a parameter, local, capture or temporary), or a field path
within one (`p.a`; all elements of one sequence are one place), or a **scope**: a body
region (the frame, a block, a loop iteration or the temporaries of one statement), an
arena, or a callable's environment `env`.

A **loan** is a place and a kind:

- **read**: a view, such as an alias, a pattern binding, a borrowed argument, a
  borrow-only use of a part, or a local result's freeze of its source;
- **write**: a `mut` argument, reserved when evaluated and exclusive from callee entry
  ([LANG-3](language.md#lang-3-evaluation));
- **identity**: a brand ([PROV-1](brands.md#prov-1-brands));
- **bound**: the value lives in, or depends on content living in, that scope or
  parameter.

Every value carries a **loan set**. A value holding no bound loan is **global**; any other
value is **local**. Plain values hold identity loans only.

Inside a body, loan sets come from flow. Reading or borrowing a place adds a read loan on
it. Storing a value moves its loans to the storage that holds it. A call result
substitutes its declared relation ([MODE-1](#mode-1-locality)). A closure's set is the
union of its captures' sets. A value built in a region or arena holds a bound loan on it.
A copy holds the set of the value it copies, less the read loan taken on the copied place
itself, so a copy of local content stays local. A deep copy (`unique x` when `x` is not
provably unique) builds fresh blocks where its source's content lives, so it holds the
source's bound loans on arenas and regions and none on bindings or own roots. A borrowed or
`mut` parameter's value holds a bound loan on its own place, its **own root**, which
stands for its argument: a call replaces it with everything the argument reaches plus a
read loan on the argument place. A counted share is a count, not a loan.

**Last use.** A loan lasts from its creation until the last use of every value holding it,
on every path; it does not last to the end of the holder's scope. Uses are: reading,
copying, taking, borrowing or passing `mut` the holder or a part of it; passing it to a
call; storing it; returning it; capturing it; and the implicit destruction of a holder
whose type has user-visible destruction (a resource, or an aggregate, generic, erased or
closure type that may contain one) on every exit path where it is destroyed: normal exit,
`?`, `return`, `break`, cancellation and failure. Destroying a holder without user-visible
destruction ([DROP-1](failure.md#drop-1-values)) is not a use, and writing a new value into
a holder is not a use of its old value.

**Conflicts.** An action conflicts with a loan that is live at that point when their places
overlap. A prefix overlaps its extensions, distinct fields do not overlap, all elements of
a sequence overlap, and different variants of one `Packed` payload overlap. A write, move,
reassignment, `mut` argument or `take` of a place conflicts with a read, write or identity
loan on it, except that an identity conflict ends the brand instead
([PROV-1](brands.md#prov-1-brands)). Any access conflicts with a write loan. The exit of a
scope conflicts with a bound loan on it, so no live value outlives what it reaches. The
diagnostic is reported at the conflicting action.

```hirsel
fn view(xs: List[u64]) -> List[u64] in xs { xs }

fn grow(mut xs: List[u64]) {
  let v = view(xs)
  let n = list.len(v)        // last use of v: its read loan on xs ends here
  list.push(mut xs, n)       // so the exclusive loan does not conflict
}
```

Moving `let n = list.len(v)` after the push makes the push conflict with the live read
loan (MODE-1 at the call, since a local result holds the loan).

**Derived notions.** A physical pattern binding holds a read loan on the matched place. An
environment-local result holds the closure's set; it is global when every captured content
is global ([CLO-1](closures.md#clo-1-scoped)). A temporary holds a bound loan on its
statement's temporary scope. The receiving contract of body-local storage is the relation
fixed at its declaration ([LANG-15](language.md#lang-15-bindings)), and that of
caller-owned storage is its declared relation; neither is inferred from later stores.
Crossing requirements belong to receiving contracts; crossing evidence belongs to current
content. Changing only the lifetime of access does not erase a referent's evidence, and
preserving that evidence does not change the access lifetime.

**Uniqueness is not a loan.** Uniqueness is a count fact. A share ends a binding's
uniqueness for the rest of its scope even though loans end at last use; ending uniqueness
at a share's last use would need counts released at last use.

### MODE-1: Locality

A value is **global** (it may be stored anywhere and outlive the call) when it holds no
bound loan, and otherwise **local**, bounded by the places it names. Locality is written
with one **relation**, `T in S`. `S` is one place or a braced set (`in x`, `in {x, a}`)
drawn from parameters and their field paths (`p.a`), bindings in scope, arenas, `heap`,
and `env` on a closure or function type. `T in S` means the value's lifetime is bounded by
`S`, it may reach only `S`, and if `S` names an arena, fresh blocks go to that arena
([PROV-4](brands.md#prov-4-arena-placement)). `heap` stands alone: `in heap` is the empty
relation, global. A value is **within** `S` when each bound loan it holds is on a place
`S` names, on a subplace of one, or on a place that the relation of a parameter in `S`
names; read loans on a body's own places end when it returns.

**Results.** A result without a relation is global. For `-> T in S` the body's result must
be within `S`, so `in p.a` admits a value bound by `p.a` but not one bound by `p.b`. At a
call, each named parameter is replaced by its argument at the same field path, and the
caller passes the destination; constructing a block in it is an allocation site
([MEM-5](memory.md#mem-5-allocation)). Until the last use of every value holding the
result's loans, the caller treats the named argument places as borrowed. A global
argument lends no bound loan, so a getter over a heap map returns an ordinary global
value, which still freezes the map while it is used. A taken parameter named in `S` is
consumed: no read loan remains, and the result is as local as that argument was. A named
function has no environment: `env` in its result relation is MODE-1 at the relation.

```hirsel
fn first[T: copyable](xs: List[T]) -> Option[T] in xs {
  if list.len(xs) == 0 { return None }
  Some(xs[0])
}

fn escape(xs: List[u64]) -> List[u64] { xs }   // MODE-1: a borrowed xs is local to the caller
```

**Borrowed and `mut` parameters** are local to themselves. Inside the body, `x: T in S`
reaches `x` and `S`: a written `S` adds to `x`'s own root and never replaces it. The
caller's argument must be within `S`, apart from the read loan the call takes on its
place; with no relation, any argument is accepted. For `mut x: T in S`, `S` is also the
fixed receiving contract of the caller's storage: everything stored into `x` or its parts
must be within `S`, so a `mut` parameter without a relation receives only global values.

**Taken parameters** are global unless related: `take x: T in S` reaches only `S` and its
argument must be within `S`; `take x: T in x` is local to its own argument.

**Bindings.** `let x: T in S` fixes `x`'s receiving contract
([LANG-15](language.md#lang-15-bindings)); `S` may name places in scope, and a binding
named in its own relation stands for its own scope.

**Function types and closure results.** `fn(..) -> T in S` may name that callable's
parameters and `env`, its environment. A call through such a value replaces `env` with the
callee value's own loans and the parameters as above. `in env` alone is accepted only when
no parameter of that function type is non-plain under the `plain` bound of
[GEN-1](generics.md#gen-1-bounds) (an `Arena` or other scoped capability is non-plain);
otherwise it is MODE-1 at the relation, and the diagnostic lists the non-plain parameters
as candidate sources (`in {env, x}` or `in x`).

**Shared.** A borrowed `Shared` handle is bounded like any borrowed value; only a copy of a
`Shared` root, which acquires its own atomic count, is global
([MEM-11](memory.md#mem-11-shared)).

Local values cannot be stored into storage whose contract they exceed, returned under a
relation that excludes their places, or captured by escaping closures. A relation on a
plain type is inert, and relations are not written on part types
([OWN-8](#own-8-parts)).

All call results are owned. A result relation substitutes only the selected sources;
selected global heap values lend no lasting source even when another argument makes the
result local. Non-global borrowed and `mut` values lend their bounds plus their binding
home; copies of non-flat local content keep lasting source bounds after access loans end.
Plain results are independent bits.

A `sendable` requirement checks the content lent by the caller and stays available as
evidence through the resulting view; it does not remove the view's own root or any other
bound loan. A view cannot escape or be transferred as owned content on the strength of
that evidence.

### MODE-2: Uniqueness

A `unique` value exclusively owns every counted block reachable from it; a `Shared` root
inside it is a boundary (its contents are not owned and never traversed). Fresh
constructions from unique parts are unique; `unique x` deep-copies when `x` is not
provably unique (an allocation site). Copying, sharing a part, or passing as a plain `mut`
argument ends a binding's uniqueness; borrowing does not, including a borrow-only use of a
part of any type ([OWN-8](#own-8-parts)). A `mut x: unique T` parameter accepts only unique
arguments and only unique whole-value assignments, so its root is updated in place.
Without `unique`, in-place update still happens whenever the run-time count allows
([OWN-7](#own-7-uniqueness-test)).

Uniqueness describes actual owned contents, independently of how fields are declared.
Taking or destructuring a currently unique owner keeps uniqueness on what it yields
through nested fields up to a `Shared` boundary; copying non-flat parts or sharing an
intermediate owner weakens it. Fresh records, variants, tuples, sequences and boxes derive
uniqueness from owned unique transfers, plain bits, flat copies and `Shared` boundaries.
Projected plain writes preserve it; aliased counted replacements end it. A borrowed or
`mut` counted local-result share ends its source's uniqueness, and so does a closure call
whose counted result may share a borrowed counted capture. A `unique` annotation on a
`Shared` handle is inert: it promises no exclusive atomic count. Read-only reception may
weaken nested uniqueness along owned payloads; mutable hand-back, callable headers and
interior-mutable or channel contracts are invariant, and weakening a nominal generic
requires `data`. Callable promises, erased captures and cleanup summaries follow the
variance of [CLO-2](closures.md#clo-2-transparent).

When a sendability proof depends on exclusive counted ownership, an operation that ends
that ownership also ends that proof, including through an enclosing aggregate. A
`sendable` qualifier does not make counted copying preserve sendability, and crossing
evidence kept through a receiving default is not permission for unique mutation.
Inferring a qualified element type for a generic mutable parameter preserves that
element's exact receiving contract. A container operation preserves qualified element
receipts through any internal move, growth or copy-on-write. Read-only weakening used to
call a copying getter does not change the source storage's fixed contract and does not
preserve evidence lost by a returned share.

### MODE-3: Once

A `once` function value may be called at most once and is noncopyable. A closure that
takes a capture, owns a noncopyable capture or captures a once function value is once.
Borrowing a noncopyable resource or capability read-only does not make a scoped closure
once, and borrowing a once function gives no permission to call it. Task closures and
`task.scope` bodies are once.

`once` may be written only on a known function type, including through transparent
aliases, not on an unconstrained `T` or a scalar. An unqualified `T` can still carry a
complete once actual.

### MODE-4: Sendable

A value may cross tasks only if it is sendable: plain values; `flat` values (copied);
unique values whose contents are sendable (moved); `Shared[T]`; Senders, Mutexes and
Atomics, which name run-owned handles and cross only within their scheduling domain
([CAP-2](capabilities.md#cap-2-charged-at-use)); regions with sendable roots; and global
capabilities whose complete represented value meets these rules. Not sendable: counted
values that are not unique, Receivers (they cross only as task-closure captures), local
values, scoped capabilities, and task handles.
Locality wins even for flat values: a local value never crosses.

A written `sendable T` is a receiving requirement for the complete value of `T`, for every
type `T`. Every argument, initialization, constructed part, result and replacement
entering that position must prove it from its actual content and modes, before receiving
defaults weaken them; writing the qualifier never supplies its own proof. A successful
receipt carries the proof into the receiving value, including when `T` is generic or
contains erased environments. The requirement is fixed, while the proof describes current
content: sharing counted storage, changing a part or another weakening operation
recomputes or invalidates it, and a later use that needs sendability must prove it again.
A move, or a borrow that preserves content, preserves the evidence; a counted copy does not
inherit it from its source's written type. The evidence grants no unique mutation,
copying, authority or longer lifetime.

A borrowed or mutable view keeps the referent's checked crossing evidence for use within
its permitted lifetime. Checking a receipt uses the actual content before the receiving
call's own loan is added; forwarding a view may use the kept evidence but cannot discard
any existing content or placement bound. Crossing tasks still checks the operand's own
locality and ownership: local values do not cross, even when their referent or erased
environment has sendable evidence.

A closure or `Iter` carries crossing evidence exactly when everything kept in its
environment meets the crossing rules for its captured ownership; the evidence is computed
where it is built and kept in its complete value type, independently of the lifetime of a
view of it. An aggregate's proof includes its owned parts and their erased environments,
but not a callable's parameter or result types.

The `sendable` generic bound tests the actual complete value type, including locality,
uniqueness and capture summary, before parameter defaults weaken modes. Instantiation
keeps that evidence for sendability obligations; it grants no unique mutation through an
unannotated parameter. Repeated occurrences of a type parameter meet their obligations
independently: a unique argument cannot make another, aliased argument sendable.
`sendable T` on an occurrence of an unbounded type parameter requires and supplies
evidence for that occurrence only; it adds no bound to the parameter. A symbolic
obligation is met only by evidence that follows from the obligations available in the
same binders.

`Ptr` and `FnPtr` are plain and therefore sendable; this grants no `Unsafe` capability and
no lifetime for the pointee or foreign context. The Receiver task-capture exception waives
sendability only, never locality. A `mut` parameter accepts the storage's current value
and must preserve its receiving crossing contract on hand-back, at the root and every
part; stronger current evidence alone does not justify lending storage under a different
fixed contract. Transporting a qualified generic actual does not prove an incoming value
sendable: each input, result, stored part and mutable hand-back must meet its
instantiated requirement with current evidence, and copying a counted value or an
enclosing owner invalidates every crossing proof that depended on the ownership lost. A
generic instantiation, written type, expected result or later projection cannot restore
such a proof.

### MODE-5: Closure rule

A closure's modes are the weakest of its captures': capturing a local value makes it
local (scoped; its loan set is the union of its captures' sets), taking a capture or
capturing a once value makes it once, capturing a non-sendable value makes it
non-sendable, and its authority summary ([CAP-2](capabilities.md#cap-2-charged-at-use)) is
the union of its captures'. Multiplicity follows capture ownership, not the borrowed
resource's type alone. Borrowed non-plain captures are local views inside the body. The
current captured content, not only the declared receiving rows, decides whether an
environment-local result specializes to global ([CLO-1](closures.md#clo-1-scoped)).

### MODE-6: Scoped capabilities

A capability is either global (`Dir`, `MonotonicClock`, `WallClock`, `Net`, `Stdout`,
`Tasks`) or scoped (`Tx`, `Scope`, `Arena`, `Test`; always local). Its declaration says
which. A scoped capability received by a closure cannot be returned, stored or captured by
an escaping closure, so it cannot leave the call that created it.

A user capability is declared on an opaque type with `@capability(global)` or
`@capability(local)`; these are contextual attribute names, not reserved words. A global
capability may be kept subject to its contents' ordinary ownership and lifetime rules;
primitive global capabilities are plain. A source capability declaration must prove every
exported structural bound from its representation. A global capability's representation
cannot hold scoped capabilities or scoped content. A local capability is non-plain and is
always bounded by its creating invocation and every scope its contents reach; a
declaration cannot remove those bounds.

### MODE-7: Inference

Modes inside bodies are inferred; modes on signatures are declared
([LANG-6](language.md#lang-6-signatures)). Defaults: borrowed and `mut` parameters are
local to themselves, taken parameters and results are global (no relation), everything is
aliased, and closures are many.

## Passing modes

| Form | Rule | Meaning | When the call fails |
|---|---|---|---|
| `x: T` | [OWN-1](#own-1-borrow) | Borrow (the default) | The caller keeps it |
| `take x: T` | [OWN-2](#own-2-move) | Move | The callee owns it |
| `mut x: T` | [OWN-3](#own-3-exclusive-access) | Exclusive in-place access | The caller keeps it; no rollback |
| ordinary | [OWN-4](#own-4-copyable) | Copyable | |
| noncopyable | [OWN-5](#own-5-noncopyable) | Moved or borrowed only | As above |

### OWN-1: Borrow

A parameter without a passing mode is borrowed: local and read-only for the call. A
`plain` value is copied by reading it; anything else derived from `x` is local to `x`
([MODE-1](#mode-1-locality)). A borrowed value cannot be taken, and storing a borrowed
resource into any storage is OWN-1.

Returning borrowed storage obtains an owned copy or share only when it is copyable; a
concrete borrowed noncopyable extraction is OWN-1. For an implicit read of a selected
result, a generic parameter occurring in a result whose relation names a borrowed `x` has
a signature-derived copyable obligation, and every call proves that its actual is
copyable ([GEN-1](generics.md#gen-1-bounds) otherwise). This is permission for result
flow, not a bound inferred from the body: an explicit `copy`, unrelated duplication, a
many capture and every other operation that needs copying still need a written bound.
Results selecting a `mut` or taken source acquire no such obligation; a `mut` getter that
shares `T` must declare `T: copyable` itself. Fresh or taken resource results are allowed,
subject to lifetime.

### OWN-2: Move

`take x: T` moves the argument; the caller writes `take v`. Using a value after it has
moved is a compile error.

```hirsel
fn keep(take xs: List[u64]) -> List[u64] { xs }

fn caller() -> u64 {
  let xs: List[u64] = [1, 2]
  let kept = keep(take xs)
  list.len(kept)               // list.len(xs) here would be OWN-2
}
```

### OWN-3: Exclusive access

`mut x: T` gives exclusive in-place access, written `mut x` at the call site; fields and
elements are places (`mut r.f`, `mut xs[i]`). No storage is passed twice: distinct fields
are separate storage, a loan lasts until its holder's last use, all elements of one
sequence are one storage, and a scoped closure's captures count as arguments of the call.
Whole-value assignment is allowed ([MODE-2](#mode-2-uniqueness) for `unique`). Mutable
callable parameter types are invariant in once and many.

```hirsel
fn both(mut a: List[u64], mut b: List[u64]) {
  list.push(mut a, 1)
  list.push(mut b, 2)
}

fn caller() {
  let mut xs: List[u64] = []
  let mut ys: List[u64] = []
  both(mut xs, mut ys)         // both(mut xs, mut xs) is OWN-3
}
```

A replacement resolves and checks its destination address and prepares the right-hand
owner in source order, installs the new contents, then finalizes the displaced contents.
A failed address check preserves both owners; a failure in the old value's finalizer still
sees the installed replacement. Different `Packed` variant payloads overlap; distinct
fields of one active payload are separate. Receiving permissions travel with the installed
whole or field values, not the displaced ones: a refill from the same source keeps its
loan, and a fresh refill can end the final holder's loan.

### OWN-4: Copyable

`plain` values copy implicitly; `flat` values copy with `copy x` (a memory copy); other
copyable values copy with `copy x`, which shares storage. The first mutation of shared
storage detaches it, which is an allocation site.

### OWN-5: Noncopyable

Resources (Senders included), task handles, Receivers, once closures, arenas, regions and
anything containing them are noncopyable: they are moved or borrowed only.

### OWN-6: Crossing tasks

Tasks never share counted blocks, except the root of a `Shared[T]`. Only sendable values
cross ([MODE-4](#mode-4-sendable)): plain and flat values are copied; unique values move,
and their blocks become the receiving task's; a `Shared` root is counted atomically. A
child task borrows nothing counted from its parent. Every other count operation is plain
and task-local, and there is no marking walk.

### OWN-7: Uniqueness test

A block whose count says it has one holder may be mutated or reused in place. Region and
arena blocks, and blocks inside a `Shared`, carry no count. Published region and `Shared`
blocks are never mutated; the unpublished builder phase
([MEM-10](memory.md#mem-10-regions)) and the unique arena builder root
([MEM-4](memory.md#mem-4-arenas)) are the only construction exceptions. Mutating a nested
arena block still detaches it to the heap.

### OWN-8: Parts

A `box` payload is a part: borrow-only inspection is a view, an owned read shares the
selected part, and whole destructuring with `take` of a uniquely owned box moves its
payload's parts. Writes through a box obey the same replacement and copy-on-write detach
rules as other part writes.

Reading a field or element as an owned value shares that part (a count increment, or a
copy if plain) whatever the owner's mode, and needs it to be copyable. A borrow-only use of
a part of any type (passed to a borrowed parameter, compared, read through or iterated
without taking) is a view: it holds a read loan on the part's place, acquires no count and
does not end uniqueness. Its locality follows the owner and current content, keeping all
source bounds. Flat parts copied from borrowed storage stay local, including through
computed block roots.

Part types (record fields, variant payloads, tuple elements, `box` payloads and type
arguments) carry no relation; a function type in a field keeps its own result relation
(`f: fn() -> T in env`), which is not a relation on the field value. Physical projections
and patterns keep the contracts their declarations own; bound names describe what was
transferred to them. Writes obey fixed receiving contracts. To move a part out of an owned
value, destructure the whole (`let Rec(items: items, ..) = take r`), subject to
[LANG-13](language.md#lang-13-patterns) for types with `drop`. A whole resource
destructure suppresses the parent finalizer; bound fields move and omitted fields drop at
the destructure in declaration order.

```hirsel
type Holder { items: List[u64] }

fn store(take xs: List[u64]) -> Holder {
  Holder(items: xs)              // a borrowed xs here would be MODE-1
}

fn contents(take h: Holder) -> List[u64] {
  let Holder(items: xs) = take h
  xs
}
```

## On `copy` and `take`

`copy x` shares `x` and keeps `x`'s bounds, so it never makes local content global; the
one exception is a copied `Shared` root. When a value must escape and the caller no longer
needs it, `take` is the repair: declare the parameter `take x` and pass `take v` at the
last use. It costs nothing and moves ownership. When the caller keeps using the value,
`unique x` deep-copies it (an allocation site). A `copy` that changes nothing may be
reported as redundant.
