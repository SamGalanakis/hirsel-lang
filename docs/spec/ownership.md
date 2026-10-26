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
source's bound loans on arenas and regions and none on bindings or own roots. A function
value's borrowed captures are not copied, so a deep copy of a function value, or of a type
parameter's value, keeps the loans its source holds. A borrowed or `mut` parameter's value
holds a bound loan on its own place, its **own root**, which stands for its argument: a
call replaces it with everything the argument reaches plus a read loan on the argument
place. A counted share is a count, not a loan.

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

Rebinding a [borrow cursor](language.md#lang-15-bindings) writes its view-holding
binding, not the storage reached through the view. Read loans held by ordinary aliases
and physical data-pattern bindings protect the underlying storage independently of
later cursor rebindings. Replacement removes only the cursor's old holding of those
loans. Other holders, actual loans on the cursor binding (including scoped captures),
and inherited bounds remain subject to last use and ordinary conflict rules.

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

**Uniqueness is not a loan.** Uniqueness is a count fact. An ordinary counted copy ends a binding's
uniqueness for the rest of its scope even though loans end at last use. The confined
related-result exception in MODE-2 requires an actual release of every dependent holder;
last use alone does not release a count.

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

**`mut` content at the caller.** A body treats the content of a `mut` parameter without a
relation as global, and it may move that content from one such parameter into another or
into its result. So at a call, the result and each argument passed to a relation-free
`mut` parameter are afterwards also bounded by the content of every argument passed to
such a parameter whose type can hold a part of theirs: its lifetime, the places it reads,
and the arenas and regions it is placed in
([PROV-4](brands.md#prov-4-arena-placement)). The access loan on the argument's own place
is not content, and inside a body a `mut` parameter's content is its written relation, else
global. This asks nothing of the arguments: an arena-placed map passed to
`sorted_map.insert` keeps working, and what `sorted_map.remove` gives back stays in its
arena. A plain result holds no content, and storage whose type can hold no part of the
content (a `List[u8]` beside a `List[String]`) keeps its own bound. A checked public
`plain` bound proves no counted content even when an opaque representation is hidden.

```hirsel
fn pop(mut xs: unique List[String]) -> Option[String] { list.pop(mut xs) }

fn escape(a: Arena) -> Option[String] {
  let inside: String in a = "text"
  let mut xs: unique List[String] in a = [inside]
  pop(mut xs)                  // PROV-4: the popped value lives in a
}
```

**Taken parameters** are global unless related: `take x: T in S` reaches only `S` and its
argument must be within `S`; `take x: T in x` is local to its own argument.

**Bindings.** `let x: T in S` fixes `x`'s receiving contract
([LANG-15](language.md#lang-15-bindings)); `S` may name places in scope, and a binding
named in its own relation stands for its own scope.

A borrow cursor checks each incoming view's storage anchors against its fixed origin
cover, even when the viewed content is global. Admission does not extend an origin's
lifetime. A later view may return to any admitted parent or sibling place; it need not
be within the cursor's immediately preceding view.

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

Destructuring an owned result or moving it through local bindings preserves that
ownership; `in m` describes its lifetime sources, not borrowed access to `m`.
Neither a larger cursor cover nor `take` changes this. A read-only lookup may
instead traverse borrowed entries within the cursor's fixed cover, as in the
[complete length-lookup recipe](language.md#lang-15-bindings). That recipe takes
O(n) traversal in the worst case and preserves the lookup's `0` miss result;
it is appropriate only when that traversal cost is acceptable. Ordinary owned
state retains the hash-lookup algorithm when owned results are intended.

**Relations compose.** A local binding of the calling body (not a parameter, capture or
temporary) whose value is related to other places (a call result related to its
arguments, a borrowed view, a field read through its owner) lends a related `data`
result ([GEN-1](generics.md#gen-1-bounds)) those places' bounds, and neither its binding
home nor a read loan on itself, unless the result is related to a field path of it. The
result reaches only content the binding reaches, never the binding's storage. So a result
in `first`, where `first` is in `data`, is in `data`, and may outlive `first`:

```hirsel
fn skip(data: Bytes) -> Result[Bytes, u64] in data {
  if bytes.len(data) == 0 { return Error(0) }
  Ok(bytes.slice(data, 1, bytes.len(data)))
}

fn skip_twice(data: Bytes) -> Result[Bytes, u64] in data {
  let first = skip(data)?
  Ok(skip(first)?)                        // within data; `first` may end or be reassigned
}
```

An owned binding, including any copy (`let y = copy x`), lends only itself: a copy is
semantically a fresh value, whether or not an implementation elides the deep copy, so a
result related to `y` stays local to `y`. A result that is not `data` (a callable, an
iterator or an opaque value) may reach the binding's storage and keeps its binding home.

A `sendable` requirement checks the content lent by the caller and stays available as
evidence through the resulting view; it does not remove the view's own root or any other
bound loan. A view cannot escape or be transferred as owned content on the strength of
that evidence.

### MODE-2: Uniqueness

A `unique` value exclusively owns every counted block reachable from it; a `Shared`,
`Mutex` or `Atomic` root inside it is a boundary (its contents are not owned and never
traversed). A copied handle part therefore preserves a construction's uniqueness. Fresh
constructions from unique parts are unique; `unique x` deep-copies when `x` is not
provably unique (an allocation site). Copying, sharing a part, or passing as a plain `mut`
argument ends a binding's uniqueness; borrowing does not, including a borrow-only use of a
part of any type ([OWN-8](#own-8-parts)). A plain `mut` argument that is a part ends its
owner's uniqueness, since the callee may install a share there. A `mut x: unique T` parameter
accepts only unique arguments and only unique whole-value assignments, so its root is
updated in place, and it hands back a unique value: on every return and at every
evaluation that may fail, `x` must be unique, or MODE-2 is reported there. Releasing a value
with user-visible destruction is such an evaluation, since a finalizer may fail
([FAIL-3](failure.md#fail-3-what-runs)): a block's end, a statement's end or an assignment
that drops a resource or a once callable. A copyable value's release runs no code
([DROP-1](failure.md#drop-1-values)). A counted
result related to `x`, such as `map.get(x, k)`'s, holds a share of it and ends its
uniqueness while the result may live; its shares stay local to the borrow, so `x` counts
as unique again once no live binding and no temporary of an unfinished statement holds a
value derived from that result (a temporary lives to the end of its statement). In a
straight-line direct named call, a dead confined holder of concrete copyable owned data
may be released after its last dependent statement, before the next uniqueness demand.
This excludes ordinary copies, escaping takes, handles anywhere in the result,
opaque or abstract payloads, callable/iterator environments and ambiguous joins or loops;
existing scope cleanup still applies to those holders. Release must precede restoration
and preserve cleanup order (G11, option B of `unique-restoration-design-18/DESIGN.md`).
Uniqueness is also restored at a
return whose value derives none of them (`unique value` copies a borrow into fresh storage
that derives nothing from it), and at a failure site. A share passed by `take` to a call,
or produced by calling a function value, may be kept anywhere, so it ends `x`'s uniqueness
for good. The caller keeps treating `x` as unique after a failure too: a
`mutex.with` body that fails after a plain `mut` call installed a share in its unique
payload would leave that share in the mutex, and the next holder would update it in
place. Without `unique`, in-place update still happens whenever the run-time count allows
([OWN-7](#own-7-uniqueness-test)).

A call argument is **currently unique** when it is unique at callee entry, after every
argument has been evaluated. A lent place is passed as itself, and a `mut` one is only
reserved while the later arguments are evaluated ([LANG-3](language.md#lang-3-evaluation)),
so a later argument may still copy or share it:
`replace(mut a, take { list.push(mut keep, copy a); [] })` receives an aliased `a` and
returns an aliased list.

A part reached by field and element steps from a currently unique owner, up to a `Shared`
boundary, is itself unique, so `mut st.items` may be passed to a `mut` parameter declared
`unique`; the owner stays unique when the part comes back unique. A callable may declare
`@preserves_unique` ([CLO-2](closures.md#clo-2-transparent)) on a `mut` or `take`
parameter: a call whose selected argument and every other non-plain `take` argument that
is not a handle root are currently unique hands the selected `mut` argument back unique, or returns a unique result
for a selected `take` parameter. Other `mut` arguments need not be unique, since the body
is proved with them aliased (a bodyless core declaration, proved by nothing, needs them
unique too), so `bufio.read_until(mut reader, mut stream, .., mut out, ..)` hands a fresh
`out` back unique whatever the reader. So `list.push`, which declares it, keeps a list built
from a literal and unique elements unique, while a list copied first, or given an aliased
element, stays aliased.

A private function with a body, excluding `extern` declarations and `@test` entries,
carries a body-checked uniqueness summary. For each `mut` parameter it records the
`preserves_unique` promise that the body proves under the same entry and failure-exit
rules as a written annotation. It also records unique evidence independently for the
result root and each direct payload row of `Result`, `Option` and tuples. Every return
must prove a recorded result row. Summaries are the greatest fixed point of each
same-module call-graph component: begin with all candidate bits, re-prove the bodies,
drop failed bits and repeat until stable.

Only a direct call in the defining module uses this evidence. A function value keeps
its declared header; callable variance and generic inference use that header too.
Exported functions and calls across modules keep their declared contracts. H publishes
inferred bits separately from callable types, and V independently re-proves each bit
against the callee body, assuming its peers' published headers. A body change that loses
a bit invalidates the helper's callers. A resulting caller diagnostic names the helper
and labels the body site that lost the proof.

```hirsel
fn add(mut xs: List[u64]) { list.push(mut xs, 1) }
fn code(n: u64) -> String { "x" }
fn need(take xs: unique List[u64]) {}
fn need_s(take s: unique String) {}
pub fn main() {
  let mut xs: unique List[u64] = []
  add(mut xs)
  need(take xs)
  need_s(code(1))
}
```

Both hand-offs check from their private helpers' bodies. Installing a counted share in
`add` loses its preservation bit. A `pub add` in another module supplies no inferred
promise, and calling `add` through `let f: fn(mut List[u64]) -> () = add` supplies none.
A helper that installs a share before a possibly failing evaluation loses the bit even
if it repairs the value afterwards. A producer returning a counted constant, such as
`const NAME: String = "x"; fn code() -> String { NAME }`, has no unique-result evidence.
In a recursive component, a share-installing member loses its bit and re-proof removes
any dependent bits in its peers. These six cases remain rejected at a subsequent
uniqueness demand, including when inferred bits are forged into TIR.

A call is **parametric** when it directly calls a generic function whose owned result holds
no counted content except values of type parameters it has no copying permission for
([GEN-1](generics.md#gen-1-bounds)). Such a body only moves those values, so the result
holds exactly values its arguments supplied, and it is unique when every possible supplier
is. The suppliers are each taken or `mut` argument whose type names such a parameter, which
must be currently unique or a `Shared`, `Mutex` or `Atomic` handle (a handle root owns one
count on a run-owned cell, and no copy or update of the result reaches the cell's
contents), and each function argument that receives them only by `take`,
whose instantiated result must prove uniqueness by its rows: written `unique`, or a tuple
whose every part does or is plain, since a tuple is inline and owns no block of its own. A
record or variant result counts only when written `unique`, since its declaration may hold
unqualified counted fields the function type does not show. Such a function argument may be
lent values of other parameters: the callee cannot hand it a held value, so the held values
the call returns are still exactly what it returned, each proved unique by its own body.
`mutex.with[T, R]` lends its body the guarded `T` and returns its `R`, so a body declared
`-> unique R` makes the call's result unique, and a body that builds that result from a
part of the lent `T` fails its own proof.

Which engine checks what: the source checker and the IR checker both take a callee's
parametric shape from one table (`hirsel_types::parametric_inputs`) and a function
argument's result proof from one row test (`unique_rows`), so a fault there is common to
both; each derives an argument's uniqueness at callee entry from its own flow. Checked
execution judges neither: it counts each block's live holders, so a value reaching a
place the IR declares `unique` (a local, a parameter at entry, or a native operation's
parameter such as `shared.freeze`'s) traps when another live holder shares one of its
blocks, whatever was proved ([implementation](implementation.md#checked-execution)), and
the checked heap executor traps a write into a frozen graph whose counts exceed one. A
false `unique` claim the IR never declares, such as an elided deep copy whose result only
an aliased place receives, has no run-time witness.

Two kinds of argument name such a parameter and supply nothing. A borrowed argument whose
type names one only in data: a checked body cannot move a value out of a borrow
([OWN-1](#own-1-borrow)) or copy one ([GEN-1](generics.md#gen-1-bounds)), and what it
derives from the borrow stays local to it ([MODE-1](#mode-1-locality)), which the global
result excludes; every core operation that hands back content of a borrowed argument
relates its result to it, and another checked callee is held to the same rules. And a
callback lent such a value that has nowhere to keep a copy: its type is `uses {}`, its
result and error name no type parameter and are plain, and none of its `mut` parameters
holds counted content, while its environment is fixed when it is built
([CLO-1](closures.md#clo-1-scoped)). Some arguments would hide where a value goes, so they
make the call ordinary:
- a borrowed argument of a bodyless core or foreign declaration that names such a
  parameter, since that declaration may keep or hand back a share of it
  ([CLO-2](closures.md#clo-2-transparent));
- any other callback lent such a value, which could keep a copy;
- a handle, iterator or function that holds such a value inside a non-function argument,
  including in a part of a nominal declaration that names a parameter the argument
  instantiates with it; an opaque nominal type, whose parts are hidden, counts as one.

Any other call is an ordinary call, whose result holds its declared modes: a result declared
as a bare type parameter `T` is aliased even when a `unique` payload instantiates `T`.
Parametric calls let `results.unwrap_or(results.map(take x, keep), ..)`, with
`keep: fn(take unique Bytes) -> unique Bytes`, hand a unique payload through unchanged. But
`unwrap(t, make())`, with `make() -> Result[unique Bytes, ()]`, yields an aliased `Bytes`,
because `make`'s result is not itself unique.

Uniqueness describes actual owned contents, independently of how fields are declared.
Taking or destructuring a currently unique owner keeps uniqueness on what it yields
through nested fields up to a `Shared` boundary; copying non-flat parts or sharing an
intermediate owner weakens it. An owned call-result receipt has an unshared outer root,
independently of its deep uniqueness. Consuming that receipt with `?` or a `take` match
preserves its declared unique payloads; a local holding it keeps that evidence until the
root is shared. This does not prove uniqueness of unqualified counted payloads. Taking
an unqualified counted payload does not carry the receipt's outer-root evidence into
that payload. Fresh records, variants, tuples, sequences and boxes derive
uniqueness from owned unique transfers, plain bits, flat copies and `Shared` boundaries.
Projected plain writes preserve it; aliased counted replacements end it. A borrowed or
`mut` counted local-result share ends its source's uniqueness, and so does a closure call
whose counted result may share a borrowed counted capture. A `unique` annotation on a
`Shared` handle is inert: it promises no exclusive atomic count. Read-only reception may
weaken nested uniqueness along owned payloads; mutable hand-back, callable headers and
interior-mutable or channel contracts are invariant, and weakening a nominal generic
requires `data`. Callable promises, erased captures and cleanup summaries follow the
variance of [CLO-2](closures.md#clo-2-transparent).

For example, `csv.parse_owned` returns `Result[unique List[List[String]], CsvError]`.
Consuming its receipt with `?` or a taken match yields a deeply owned table.
Ordinary borrowed validation preserves that evidence, so `shared.freeze(take rows)`
needs no deep copy. This is a producer contract, not a claim that every parser
result is unique, and it adds no ownership inference rule. Likewise,
`markdown.parse_owned` returns `unique Node`, and `parse_bounded_owned` returns
`Result[unique Node, ParseLimit]`: both construct the owned tree before returning,
rather than copying a completed unqualified result.

The one exception in a callable header is the uniqueness of a parameter or result row
itself, which follows that edge's variance, because a `unique` value is always a valid
aliased one: a function value whose result is `unique` is received where the function
type's result is aliased, and one whose borrowed or taken parameter is aliased where that
parameter is `unique`. So `invoke(fn() -> unique String { "text" })` meets
`invoke[T](f: fn() -> T)` with `T = String`, and `len(take s: String)` meets
`fn(take unique String) -> u64`. A `mut` parameter, which the callee hands back, and every
row below a header row (a `List[unique String]` result) stay exact. A rejection names
both function types, for example: the function value is `fn() -> String`, but the
required function type is `fn() -> unique String`.

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
A noncopyable value received by `take` has no other holder, since nothing can share it
([OWN-5](#own-5-noncopyable)), so it crosses exactly as a call result of its type does:
`take l: Listener of n` moves into a child as the result of `net.listen` would. A deep
copy (`unique x`) owns only fresh blocks, so it crosses whenever its type does once
unique, whatever its source.

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

Passing a proven opaque `@captures` carrier to another retaining function preserves
its current crossing evidence; the opaque declaration is not a substitute for the
environment proof established at construction.

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

A capability is either global (`Dir`, `MonotonicClock`, `WallClock`, `Net`, `Stdout`, `Telemetry`,
`Tasks`, `Runner`) or scoped (`Tx`, `Scope`, `Arena`, `Test`; always local). Its declaration says
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

Modes inside bodies are inferred. Callable type rows and exported headers are declared;
private helpers additionally carry the body-checked direct-call summaries of
[MODE-2](#mode-2-uniqueness)
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

Rebinding a borrow cursor transfers a read-only view between bindings and grants no
exclusive access or ownership. Function results and ordinary owned storage keep their
existing copy and move requirements.

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
Whole-value assignment is allowed ([MODE-2](#mode-2-uniqueness) for `unique`).
Moving out of a `mut` place requires `replace(mut place, take new)`, which returns
its old value while leaving the replacement installed. Mutable
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
and task-local, and there is no marking walk. A block's storage may return to another
thread's allocator; this is unobservable (DET-1). Checked execution records the task that
reaches each tracked block and traps when another task uses it
([implementation](implementation.md#checked-execution)).

### OWN-7: Uniqueness test

The core arena scope callback's consumable grant is an affine entry permission
([MEM-4](memory.md#mem-4-arenas)), even though its parameter retains the borrowed
`fn(a: Arena)` spelling. Ordinary borrowed Arena parameters supply no such permission.
Consuming the grant transfers only the root's own custody loan; all other live views,
including later closure captures and cleanup uses, must have ended. An explicit taken
Arena helper transfers the same permission and cannot recreate it. Branch joins retain
the possibility of consumption, so a later use requires the grant live on every path.

`Region[T]` is noncopyable and owns one DROP-2 cleanup obligation. Its complete T evidence
determines sendability: `Region[unique List[u64]]` can cross with that proof preserved;
`Region[List[u64]]` alone supplies no crossing proof. `region.with` reserves its owner
exclusively and supplies a read-only, invocation-bounded payload view with no new owning
credits. Neither taking a projected noncopyable part nor returning a nested borrowed
field or capture can turn that view into ownership.

A block whose count says it has one holder may be mutated or reused in place. Region and
arena blocks, and blocks inside a `Shared`, carry no count. Published region and `Shared`
blocks are never mutated; the unpublished builder phase
([MEM-10](memory.md#mem-10-regions)) and the unique arena builder root
([MEM-4](memory.md#mem-4-arenas)) are the only construction exceptions. Nested arena
mutation follows [MEM-4](memory.md#mem-4-arenas) and
[MEM-6](memory.md#mem-6-bounded-arenas).

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

A part or physical data-pattern binding used to initialize or refill a borrow cursor is
a borrow-only use. Its origins and access dependencies follow the underlying owner,
including through an earlier cursor value, rather than the intermediate cursor name.

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

`copy x` shares `x` and keeps `x`'s bounds, so a copy of local counted content stays
local. A value whose only counted parts are `Shared`, `Mutex` and `Atomic` handles is
an exception in every expression context: its copy owns independent counts on immutable
roots or run-owned cells and is global, even when read through a borrowed parameter.
When a value must escape and the caller no longer
needs it, `take` is the repair: declare the parameter `take x` and pass `take v` at the
last use. It costs nothing and moves ownership. When the caller keeps using the value,
`unique x` deep-copies it (an allocation site). A `copy` that changes nothing may be
reported as redundant.
