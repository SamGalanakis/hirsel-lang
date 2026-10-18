# Memory

Long-lived values are reference counted with compile-time insertion and elision of count
operations; scoped values live in compiler-chosen regions with no counts. The value graph
is acyclic, so counting needs no cycle collector. Layer 1 adds explicit arenas, flat and
packed layouts, generational slots and frozen shared values. Placement is chosen from modes
and is never observable.

### MEM-1: Counting

Global values use Perceus-style reference counting: compile-time insertion of increments
and decrements with elision, drop specialisation and size-keyed in-place reuse. Counts are
plain ([OWN-6](ownership.md#own-6-crossing-tasks)). A block is freed when its last holder
drops it. A count about to overflow makes the block immortal. A projection out of a
borrowed value is counted before the storage it points into is mutated or moved, and an
uncounted projection is never reused or mutated in place.

### MEM-2: Acyclic

The value graph reachable from safe code is acyclic: values are immutable once built,
there are no references or aliasable cells, and closures cannot capture themselves.
Channel state is a runtime object outside the value graph; a cycle through a channel
buffer is broken when its last Receiver drops ([CONC-6](concurrency.md#conc-6-channels)).
There is no cycle collector and there are no weak references.

### MEM-3: Freeing

The order in which memory is freed is unobservable; resource drops follow
[DROP-2](failure.md#drop-2-resources). Release uses a worklist linked through dead block
headers, so it uses constant native stack and never allocates; children are visited so
that resource drops run in DROP-2's order. Read-only structural operations (`==`, hash,
order) past a fixed recursion depth continue on a per-task scratch stack limited to a fixed
number of levels; exhausting it is the failure `Depth`, with ordinary cleanup.
`map.insert`, `set.insert`, `map.get` and `set.contains` hash their key before they look at
the container, so a key nested past the limit fails `Depth` at the call whatever the
container holds, an empty one included. A stored key passed the limit when it was
inserted, so comparing it with a probe key that passed cannot fail.

**Unspecified:** the normative depth and scratch-level table. The reference interpreter
permits 64 native plus 1,024 scratch edges for structural reads (1,088 in all, the root at
level zero). Finite or regular nominal type graphs have no semantic width or depth cutoff.

### MEM-4: Arenas

Layer 1. `heap` is a keyword denoting the counted heap as an `Arena`: the distinguished
global, inert, plain allocator token, not a scoped grant. `arena.with(fn(a: Arena) { .. })`
opens a named arena, freed at scope end; arenas are scoped capabilities
([MODE-6](ownership.md#mode-6-scoped-capabilities)) and hold no resources. Arena values
are built bottom-up and are immutable once built, except that a `unique` builder mutates
its root block in place; mutating a nested arena block detaches it onto the heap, except a
List, Map or Set, which [MEM-6](#mem-6-bounded-arenas) copies within its arena. Arena
blocks carry no count; the heap blocks they hold are released when the arena closes.
`copy_to(heap, x)` deep-copies out of an arena (an allocation site).

`arena.seal(take a, take root) -> Region[T]` first copies every heap block the arena holds
into the arena, so a region owns all of its storage, then turns the arena and root into a
resource that may move between tasks when `T` is sendable. Sealing is an allocation site,
consumes the arena grant and accepts a root stored there or on the heap; a sealed region
accepts no allocation. `region.with(mut r, body)` lends a read-only root and returns only
results not in `r`. Seal and freeze reject roots containing resources.

An escaping environment cannot keep non-plain arena or region content at any owned
payload depth, tuple, record, variant, box and type-argument components included (PROV-4
at the closure literal); callable headers are not payload edges.

```hirsel
fn count() -> u64 {
  arena.with(fn(a: Arena) -> u64 {
    let xs: List[u64] = [1, 2]
    list.len(copy_to(a, xs))
  })
}
```

### MEM-5: Allocation

`@noalloc` is a source-level property of every path in a definition, failure paths and
contracts included: no block may be constructed whose destination may be the counted heap,
no copy-on-write detach or deep copy not proven unnecessary may occur, and every call goes
through a signature declared `@noalloc`. Constructing a block in a result with a relation
counts, since its destination may be the heap. Region placement
([MEM-10](#mem-10-regions)) and explicit arena placement do not count. The allocation
contract is part of closure and dictionary types, and library and runtime operations
declare theirs like any other signature. Heap exhaustion is environmental
([FAIL-1](failure.md#fail-1-kinds)).

```hirsel
@noalloc
fn sum(xs: List[u64]) -> u64 {
  let mut s: u64 = 0
  for x in xs { s += x }
  s
}
```

Under `@noalloc`, `fn build() -> List[u64] { [1, 2] }` is MEM-5 at the literal.

Implicit destruction is a call for this rule, including replacements, discarded
temporaries and every exit path. Under `@noalloc`, every value that can be destroyed must
satisfy `noalloc_drop`; consuming it into another call does not remove the cleanup
obligation on earlier failure paths. Borrowing alone creates no destruction obligation. A
value transferred on every exit with no possible intervening failure creates no cleanup
obligation, so a straight generic identity can be noalloc. The diagnostic names the
binding, parameter, temporary or replacement that introduces the cleanup obligation, with
notes identifying the destructor signature.

Invocation, construction destination and implicit cleanup are independent obligations,
failure paths included. A written `take` stays caller-owned until entry. A noalloc callee
does not exempt constructing a global or retained argument or closure environment. Counted
deep `unique` extraction, copy-on-write detach and allocating user destruction need
allowance; plain payloads and `Shared` boundaries copy no owned blocks. Region-local frame
constructions and explicit arenas do not charge heap allocation, but their lifetimes still
cannot escape.

Library cleanup summaries are declared like user summaries. The cleanup effects of erased
closures and `Iter` states are carried in their complete value types, independently of
their invocation effects: noalloc, may-allocate, or a conjunction of generic
`noalloc_drop` obligations. A body-local construction may prove that summary; an exported
erased type without a declared proof conservatively permits allocating cleanup. A `mut`
hand-back keeps the cleanup summary exact at the root and every depth
([CLO-2](closures.md#clo-2-transparent)). `@noalloc` on a function type is a promise that is
proven, and an inferred closure literal carries the evidence its body proves.

### MEM-6: Bounded arenas

`arena.bounded(cap, body) -> Result[R, OutOfSpace]`: each constructor into that arena
charges a logical size, each `list.push` to a List in it charges its element's logical size
before it consumes its argument, and each `map.insert` or `set.insert` of a key a Map or
Set in it does not hold charges that entry's logical size (key plus value, or the member)
before it consumes its arguments, and each Io read into a List in it (`stdin.read`,
`file.read`, `net.read`) charges its maximum bytes before its effect, whatever it then
reads; exceeding `cap` is a failure that unwinds to this call, which returns
`Error(OutOfSpace)`, and a read that exceeds it reads nothing. A key that fails `Depth`
([MEM-3](#mem-3-freeing)) charges nothing. Spare capacity, growth relocation, a Map's or
Set's hash index and abandoned storage are not charged, so the charge is the same on every
engine. The logical size table is in the Bounded arenas section of
[the runtime README](../../crates/hirsel-rt/README.md).

A List, Array, Map or Set built in an arena (a literal, `map.empty()` or `set.empty()`
placed there as [PROV-4](brands.md#prov-4-arena-placement) says, `copy_to` or a copy made
by this rule) is **unshared**. It becomes **shared** for good when it or a value holding it
is copied (`copy`, a capture by copy, an owned read of a part, a counted local result such
as `map.get`'s), and when it is written or lent `mut` through another block (a box payload,
a sequence element, a map entry), since that block's holders are not tracked. The first
write to a shared arena List or Array (a push, an Io read, an element store, `pop`,
`remove` or `swap` that moves an element), or Map or Set (an insert of a new key, a
`map.insert` that replaces a value, or a `remove` of a held key), copies it into the same
arena as a new unshared one, charged its copied length or entry count, whose elements or
entries are then held twice and so shared, then the write proceeds; a write that fails its
bounds check, and a `set.insert` of a member the Set holds, copies nothing, and
`list.reserve` is only a hint there. A remove charges nothing and returns nothing to the
budget. A List of `Packed` values ([MEM-13](#mem-13-packed-variants)), whose storage is
sequential, is not tracked: a write to one is not charged. An unshared List grows in place,
relocating within its arena by doubling from four elements when full, so its storage is at
most four times its charged bytes plus four elements; an unshared Map or Set keeps its
entries the same way. Consequently a push or insert through a nested place
(`list.push(mut adj[i], x)` in an arena) copies the inner List, Map or Set each time,
charged; one held by a binding or an inline record field grows in amortized constant time.

### MEM-7: Layout

Records and variants are inline; a `box T` field is a heap block, required for recursive
types. Patterns, `match` and `let` destructuring, and field or element access inspect a
box's payload through an implicit projection, recursively through nested boxes. There is no
explicit unbox syntax and no implicit boxing conversion. `String`, `Bytes`, `List`, `Map`,
`Array` and escaping closures are blocks. `Inline[T, n]`, `@layout(c)`, `@packed`,
`@align(n)` and `@soa` apply to structs and enums. Generic code never boxes implicitly, so
`@noalloc` holds for every instantiation. Borrows never alias a `mut` argument, so there are
no runtime overlap checks.

**Unspecified:** the complete composition rules for `@soa`, variants and flat types, and
the native data ABI. The attribute metadata is parsed and checked.

### MEM-8: Vectors

`Simd[T, n]` has a fixed width, no width query and a named operation set; integer lanes
are checked like scalars.

### MEM-9: Unsafe

Layer 2. Raw pointers, manual memory, unchecked arithmetic, weaker atomics and foreign
functions are available only with the `Unsafe` capability, under a written aliasing
contract, and are outside the determinism contract
([DET-4](determinism.md#det-4-outside)). An aliasing contract is documentation attached to
a signature, not an executable effect annotation. The reference interpreter detects an
executed violated unchecked contract and stops with a distinct
undefined-behaviour-detected outcome carrying the operation, site, contract and prior
output. That outcome is outside DET-1 and native parity, with no cleanup guarantee; it is
detection, not a language failure.

### MEM-10: Regions

Local values are placed in task-local regions with no counts: fixed-size storage uses the
task's data stack; variable storage may share slabs or use pooled segments. An unpublished
builder reserves capacity before writing; within capacity it may mutate and bump a cursor.
On exhaustion it reserves a larger buffer and relocates the unpublished contents, or
reports its bounded arena's `OutOfSpace`; ordinary exhaustion of backing allocation is
environmental. **Publishing** means exposing a value or projection to another live binding
or call; published blocks never relocate or mutate. Further growth builds a replacement
while preserving live views.

A pool retains at most 8 MiB of idle backing storage per task at each region exit; larger
excess is released, and task exit releases everything retained. Active storage is not
capped. This is an implementation cost bound, not a language-visible allocation budget.

A region is freed at its scope's exit by resetting pointers and releasing the heap blocks
its values hold. A result with a relation is built directly in the caller's destination:
its named arena, else the caller's region. Every region or arena allocation holds a bound
loan on that scope ([places and loans](ownership.md#places-and-loans)), so no live value
outlives the storage it reaches. Placement is chosen by the compiler from modes and is never
observable.

### MEM-11: Shared

Layer 1. `shared.freeze(take x: unique T) -> Shared[T]`, for `T` inert and holding no
resources, freezes a value deeply: its inner blocks become immortal and uncounted, and only
the root carries a count updated without data races under the run's scheduler. The current
single-threaded scheduler uses plain counts; a threaded scheduler makes them atomic.
`Shared[T]` is copyable (a root-count increment), sendable
and read by borrowing; dropping the last root frees the whole value.
`shared.thaw(take s) -> Result[unique T, Shared[T]]` returns the value with ordinary counts
restored when `s` is the last root, and gives `s` back otherwise. Freeze and thaw each walk
the value and are allocation sites.

A copied `Shared` root acquires an independent share and is global everywhere,
including when read from a borrowed parameter, capture or field. An uncopied borrowed root
keeps its local view. `unique` on the root is inert; `thaw` checks the sole-holder count at
run time. Copying and deep ownership never traverse `Shared` contents.

### MEM-12: Flat types

Layer 1. A `flat type` stores everything inline as one contiguous block with no pointers:
fixed-size fields, variable-size fields (`String`, `Bytes`, `List` of flat elements, flat
records, `Packed` variants) and at most one growable field, which must be last.
Variable-size fields are fixed at construction; fixed-size fields are mutable in place. A
flat list of variable-size elements is sequential: it can be walked but not indexed. Flat
values are copied only explicitly (a memory copy) and are sendable by copy subject to
[MODE-4](ownership.md#mode-4-sendable).

Flat is a pointer-free storage property and grants no serialization. A flat type may
additionally opt into [SER-1](serialization.md#ser-1-serializable-schemas); exposing or
accepting its bytes as a serialized value uses the canonical codec of SER-2, and the
private native flat layout is not a wire format. No target ABI is implicitly canonical.

The sole growable field is spelled `items: List[T] @ growable`, where `T` has
flat-compatible layout (sized scalars or flat elements); the marker is field metadata,
never a value mode. MEM-12 rejects a growable marker on a non-flat owner, on a field that
is not a `List`, with an element whose layout is not flat-compatible, or in a position that
is not last.

```hirsel
flat type Item { sku: String, qty: u32 }
flat type Batch { id: u64, items: List[Item] @ growable }
```

Ordinary `String` and `Bytes` are flat-compatible fields, not flat values; an ordinary
record containing them inherits no flat promise. A plain value that does not opt in
qualifies only with a pointer-free representation and no capability, function, handle or
resource. Transparent aliases preserve these facts; symbolic and opaque types need a
declared proof. Flat copies own independent bytes without weakening the source's
uniqueness, but a non-plain part derived from borrowed storage stays local. `Packed[T]` is
flat wherever it is written, so a sequential `Packed` sequence needs no scan to drop.

### MEM-13: Packed variants

Layer 1. `Packed[T]` stores a variant value in its own variant's size instead of the
largest. Its payload fields may be accessed in place, as `x.Variant.field` with an
active-tag check at access, but the variant can never be replaced, except by whole
assignment of its container; a different active variant fails MEM-13. A proven matching tag
may remove the check, never change acceptance. A sequence of `Packed` values is
sequential. Ordinary variant values may change variant but expose no payload places. A
value is one of the two, never both. `Packed` constructors use the expected `Packed` type.

### MEM-14: Slots

Layer 1. `Slots[T]` gives generational ids for lifetimes that do not nest:
`slots.insert(mut s, take x) -> SlotId[T] of s`. Removing a slot bumps its generation, so a
stale id reads `None`, never another value. An id's brand follows the container's
identity: insertion and removal keep it; moving, replacing or destroying the container ends
it. A generation that would wrap retires its slot. Lists are indexed by checked integers;
there are no list handles.

A dynamic out-of-range index into a `List`, `Array`, `Inline` or `Bytes` fails MEM-14 at
the access, before extraction or replacement, with ordinary cleanup.

```hirsel
fn second(xs: List[u64]) -> u64 {
  xs[1]                          // fails MEM-14 when list.len(xs) < 2
}
```
