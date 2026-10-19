# Closures

A closure literal `fn(x: T) -> U { .. }` is either **scoped** (it lives no longer than the
call it is passed to and borrows its captures) or **escaping** (it is stored, returned or
taken, and owns its captures). Scoped closures are free: their environment lives in the
caller's region and calls through them are specialised.

```hirsel
pub fn map[T, U](xs: List[T], f: fn(T) -> U) -> List[U]                       // f scoped, transparent, specialised
pub fn lazy_map[T, U](take it: Iter[T], take f: fn(T) -> U) -> Iter[U] uses {it, f}
```

### CLO-1: Scoped

A function-typed parameter is local unless taken. A scoped closure cannot be stored or
returned; its environment lives in the caller's region and never allocates. Its captures
are borrows. It may not assign outer `let mut` names (use `for` or a fold).

```hirsel
fn apply(xs: List[u64], f: fn(u64) -> u64) -> List[u64] {
  let mut out: List[u64] = []
  for x in xs { list.push(mut out, f(x)) }
  out
}

fn shift(xs: List[u64], k: u64) -> List[u64] {
  apply(xs, fn(x: u64) -> u64 { x + k })      // borrows k for the call
}
```

A body knows about a borrowed or `mut` non-plain capture exactly what a function body
knows about a borrowed or `mut` parameter of the same type: it is a local view, and its
non-plain parts and copies stay local. Owned copied and taken captures are the
environment's own values. An inferred local result of a closure is `in env`. The closure
value specializes that result to global only when some capture is borrowed, every captured
source's current content is global throughout, and no parameter is a borrowed or `mut`
non-plain value; checking the body keeps the environment-local result. Written and
contextual result promises are not inferred. If receiving rows hide local current content,
the closure and all its holders keep an environment-local result and keep their call
dependencies, and a receiving or capture slot that promises a global result rejects it
([MODE-1](ownership.md#mode-1-locality)). Specialization keeps the closure's actual
authority and environment sendability. A closure call whose counted result may share a
borrowed counted capture ends that capture's uniqueness, as a direct share would.

A call of a closure bounds its result by the loans its captured content holds, not by the
slot it is invoked through: in `let g = fn() { let h = fn() { w }; h() }`, `h()` returns
content bounded by `w`'s sources, so it may leave `g`'s body, and `g()` stays bounded by
those sources ([MODE-1](ownership.md#mode-1-locality)).

### CLO-2: Transparent

A scoped closure uses whatever capabilities its context holds. The call's kind, authority
summary and `@noalloc` status are the join of callee and closure, so `map(xs, f)` needs no
effect variables. Inside the callee, calling a scoped closure parameter has unknown
authority unless the parameter declares `uses {..}`; unknown authority is rejected
wherever `uses` restricts. `uses` bounds the authority entering through captures, not
every capability kind used inside the frame: parameters arrive from the caller, and
permitted capability operands can derive further capabilities locally (`Io` to `Stdout`).
Evidence about a known callee comes from its signature, captures and header, never from
another implementation's body. A written unrestricted parameter does not meet a
restricted `uses`.

A declared `uses {p}` also permits forwarding `p`: instantiate the callee's `uses`
with the actual arguments, and accept when every resulting authority term is covered
by the enclosing callable's declared `uses`. An unrestricted callback remains unknown
outside its explicitly selected parameter receipt; selecting one callback never grants
another callback's authority.

**Variance of callable promises.** A `mut` hand-back is exact for the whole value type, at
its root and at every depth: the callable's `uses` and `@noalloc` promise, erased captures
and cleanup summary. A type argument of a nominal type or a non-sequence builtin (`Option`,
user generics, `Map`, `Set`, `Shared`, `Iter`, `Mutex`, …) is exact for those promises.
Sequence elements (`Array`, `List`, `Inline`), tuple, record, variant and box parts, and a
callable's result and error are read-only positions where a stronger promise serves; a
callable's non-`mut` parameters are contravariant and its `mut` parameters are exact. The
same variance applies to written sendability requirements and erased crossing promises:
read-only reception may forget a crossing requirement or use stronger proved evidence,
and mutable hand-back is exact for the incoming and outgoing values. Derived data
evidence is not itself a fixed storage contract. A requirement already entailed by the
other written modes is redundant, so `sendable` on a plain type does not create a
distinct invariant contract.

**Declared state promises.** A callable that advances or builds an erased state (an `Iter`
or a function value) can declare what it preserves. These are general declaration
attributes, open to every package; omitting one promises nothing. Each selector names one
of that callable's own parameters, optionally with an explicit field path, and a selected
state parameter is `mut` (`@preserves_unique` may also select a `take` parameter).

- `@preserves_captures(it)`: on every normal return, returned error and failure unwind,
  the selected surviving state keeps its incoming nominal type arguments, ownership mode,
  source bounds, brands, authority, multiplicity, sendability and destruction-allocation
  guarantees. It cannot install content with wider authority, a new unlisted source, a
  stronger crossing or many-use claim, or allocating cleanup hidden by the incoming
  summary. Narrowing a formal type never erases stronger actual evidence.
- `@advance_only(it)` includes preservation and permits only advancing the existing
  erased state: it cannot be replaced, cleared or rebound through a selected field on any
  path. Traversal bookkeeping may change, visited items may be transferred or dropped, and
  each `Some` payload moves to the caller. A call through an attenuated static `Iter` type
  hands the stronger actual contract back unchanged.
- `@preserves_unique(p)` on a `mut` or `take` parameter
  ([MODE-2](ownership.md#mode-2-uniqueness)): when the selected argument and every other
  non-plain `take` argument are unique at the call, a selected `mut` argument is
  handed back unique and a selected `take` parameter's call returns a unique result.
  Other `mut` arguments impose nothing, since the proof covers them aliased, except for a
  bodyless core declaration, which has no proof: its other non-plain `mut` arguments must
  be unique too.
  Borrowed arguments impose nothing, since the body can only share out of them, except
  for a bodyless core declaration, which may store a share of a borrowed argument: there
  each borrowed argument it may retain must also be Plain or Shared. One the ledger marks `@unretained`, which the operation only reads and stores
  nothing derived from (`sorted_map.insert`'s `by`, whose brand is type-level), is
  exempt. The body is checked again with the selected parameter and every non-plain
  `take` parameter entering unique, and every other `mut` parameter entering as declared
  (aliased unless written `unique`); the selected `mut` parameter must then be
  unique at every exit (MODE-2's hand-back), and for a selected `take` parameter every
  returned value must be unique. A claim this check cannot prove is MODE-2. The selection names a whole
  parameter, with no field path. It prefixes only a function declaration, never a
  function type (LANG-7), so a closure literal never receives it, and never an
  `extern`, since C has no body to prove it (LANG-7; PROG-4 trusts only a wrapper's
  kind and model), and only a direct
  call to that declaration uses it: a call through a function value hands its `mut`
  arguments back shared.
- `@captures(state, next)` on a callable returning an `Iter` or function value: the
  selected parameters (or their field paths) determine the returned environment's
  complete authority, source bounds, brands, multiplicity, sendability and
  destruction-allocation obligations, independently of its yielded item type. At
  creation, selected non-plain captures are owned through `take` or a separately legal
  copy; a borrow never owns a resource. Unselected captures are allowed only with
  independent inert, global, many and noalloc-destruction proofs. A same-shaped generic
  relay cannot conceal a third source.

```hirsel
@advance_only(it)
fn step[T](mut it: Iter[T]) -> Option[T] {
  iter.next(mut it)
}
```

Each promise is a body obligation and a callable promise, proved where the body is checked
and independently in the IR; it is never an exemption chosen by a definition's name, and no
public promise is inferred from a body. Calls through direct or indirect declarations prove
the same guarantee for the same selector before they preserve incoming evidence. A
stronger declaration satisfies a weaker receiving contract; mutable hand-back stays exact.
A claimed promise that cannot be proved is rejected: MEM-14 for `@advance_only`, CLO-4 for
`@preserves_captures` and `@captures`, MODE-2 for `@preserves_unique`.

The proof follows the selected state, a parameter of this callable with its field path,
through every reachable operation on every exit. A place meets the selection when one field
path is a prefix of the other; an element, index or variant step counts as meeting, while
another binding or a disjoint sibling field does not, and a value that views the binding
meets it whole. Then:

- a borrowed read, and a read-only capture, is always allowed;
- a copy of a meeting place is allowed only when its type is `plain`, or when the copy stays
  inert and sendable once shared; any other copy changes the state's ownership evidence;
- a write, move, `take`, store or `unique` extraction of a meeting place is allowed only at
  or inside the selection (strictly inside it for `@advance_only`) and only when the place's
  type is closed: no type parameter, callable, `Iter` or erased environment anywhere in it,
  no brand, global, many and inert, so every value of the type has the same envelope.
  Replacing the record that holds a selected field replaces the field;
- invoking a many callable stored in the selection borrows it; a once callable is consumed;
- a `mut` argument meeting the selection needs a write these rules allow, or a callee that
  declares the same promise (`@advance_only` for an advance-only selection) on that
  argument or on a field path leading into the selection;
- a mutable, taking or copying capture of the selected content is a violation.

A closure literal receives the state promises and the `@captures` selection of its expected
callable type, as it receives a contextual `@noalloc`, and its body must prove them; a
binding or slot whose written callable type states a promise accepts only a function value
that declares it. Inside a callable whose result carries `@captures`, a closure literal
written directly as the callback of a creation call whose result retains both the callback
and its state receives a preservation promise on that state, so it cannot install an
environment the selection never saw. For each state and callback pair a creation call
retains, one of these holds: the state is independently inert and the callback hands it
back sendable; the exact incoming state and callback pair is relayed; the callback's actual
type promises to preserve the whole state; or the state's type is closed and the callback
hands it back sendable.

### CLO-3: Task closures

The closure passed to `task.async(s, ..)` is once and taken. It captures sendable values
([MODE-4](ownership.md#mode-4-sendable)) by copy or `take`, and Receivers by `take`; it
borrows nothing counted. To let several children read one large value, freeze it into a
`Shared` ([MEM-11](memory.md#mem-11-shared)) and copy the root into each. Children reach a
parent's Mutex, Atomic or Sender by capturing the handle; its payload stays reachable only
through `mutex.with`, atomic operations or `channel.send`. A copied capture whose only
counted parts are `Shared`, `Mutex` and `Atomic` handles, beside plain fields, is global
wherever it was read: each handle copy acquires its own count. So a scope body's
`svc: Services { tasks: Tasks, state: Mutex[Board] }` is copied into each child, while a
record that also holds an aliased `List` is not sendable. `task.isolate`'s
closure follows the same rules. An owning environment cannot take a parameter or capture
that its creator only borrows (OWN-1). The Receiver capture exception never permits a local
capture into a global task environment.

### CLO-4: Escaping

Taken function parameters and returned or stored closures are global. They capture global
values by copy or by explicit `take` (then they are once), never borrowed counted
parameters or `let mut` names. Their type states their authority as capability types
(`uses {Dir}`) or as parameters whose authority they inherit (`uses {it, f}`); at a call,
`uses {p}` becomes the authority summary of the argument for `p`. An explicit binding type
`fn(...) -> R` supplies an escaping context; an unannotated literal binding is scoped.

In an escaping or task closure, a free `take x` selects an owned capture at construction;
other free uses copy. Capture selection includes nested closures and is checked once, at
construction. Plain reads of borrowed parameters copy and are allowed; borrowed counted
captures and outer `let mut` bindings are forbidden. A taken immutable Receiver may be
rebound `let mut` inside the child before receiving. Scoped closures borrow; an owning once
scope-body parameter may take captures without becoming global.

```hirsel
fn adder(k: u64) -> fn(u64) -> u64 {
  fn(x: u64) -> u64 { x + k }       // k is plain: copied into the environment
}
```

Returning `fn() -> u64 { list.len(xs) }` for a borrowed `xs: List[u64]` is CLO-4: an
escaping closure cannot capture a borrowed counted value.

A global environment owns the copied or taken owner incarnations and the branded
relations proved at its creation; ending or replacing the caller's bindings does not end
them. A branded value alone supplies no owner. Borrowed captures keep their actual source
identities and loans through generic and aggregate transport; `uses` does not encode
identity or lifetime. A global callable copied from borrowed storage owns a share of its
environment; a local callable copy keeps its storage and capture bounds. Nested scoped
placement cannot escape an owned environment.

### CLO-5: Specialisation

A call passing a closure to a scoped parameter is compiled for that closure, so calls
through it are direct and inlinable, except for calls inside a recursive cycle, which are
indirect. This is a cost guarantee, not an observable one.
