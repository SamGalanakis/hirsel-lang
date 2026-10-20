# Implementation

This part states what implementations must agree on and how agreement is checked: the
reference interpreter is the oracle, the conformance corpus pins every rule, an independent
IR checker re-proves what the source checker accepts, checked execution audits memory and
lifetime behaviour, and native code must match the interpreter observably. The choices
below the observable contract (data layout, calling convention, failure propagation) are
implementation design, not language rules; they may change without changing any program's
meaning. Every executable artifact an
implementation publishes is independently validated, in debug and release builds alike.

## Shared graph algorithms

Validators and passes share the pure graph algorithms in `hirsel-graph`: ordered
traversal, reachability, dominators, strongly connected components and loop-forest
feedback edges. Each validator still derives its own IR facts and edge policy
independently. These utilities carry no producer ownership or liveness answers;
sharing them is an implementation choice, not a language rule.

## Nested compiler-local region aggregates

Placement may keep a nested construction in the frame's internal region only
when every block container on every path to it is placed in that same region,
all inline containers carry uncounted fields, and no value escapes the region's
lifetime. Foreign children, mutation, calls and returns conservatively prevent
this placement. User-visible finalization is never elided.

Region block references and inline region aggregates carry no holder credits.
Their copies are plain reads; their slot drops and region payload child drops
are omitted. The region frees all of their storage together. Logical layouts
still describe the fields; the LIR representation records their uncounted use.
The existing construction drop-elision flag distinguishes these payloads from
ordinary region payloads whose cleanup remains scheduled. Nested pointer cells
retain their logical layout encoding; the emitter rebases uncounted pointers
at field stores and reads without reading a counted header.

C independently checks that a construction with elided drops contains only trivial
fields or uncounted children in the same region, propagates region provenance
through inline containers, storage and projections, and rejects a value returned
or used after its region ends. H checks the actual physical cells: no heap holder
may disappear when a region is freed, and no reference outside the region may
point into it at exit. Neither check trusts placement's escape analysis.

## The reference interpreter

The reference interpreter executes typed IR directly and is the oracle for every observable
outcome: completion with its exit status, a language failure with its rule and site, Blocked
([CONC-8](concurrency.md#conc-8-global-quiescence)), standard output and the recorded
effects. Every other engine, configuration and compilation topology must produce the same
observation for the same program and run configuration. Tooling limits (evaluator step and
call-depth guards, budgets) are reported as non-comparable tooling outcomes, never as
language results.

## Checked execution

Checked execution is an opt-in interpreter mode that audits the memory model on every
executed path. It reads only typed IR, run-time values and declared signatures,
independently of the source analyses and the IR checker. It either produces exactly the
normal execution (same outcome, output, failures, effects and observation) or stops with a
**trap**: execution stops at once, no further cleanup runs, prior output is kept, and there
is no comparable observable outcome.

| Trap | Rules | Raised when |
|---|---|---|
| region use after scope exit | MODE-1, MEM-10, OWN-1 | an access reaches a value bounded by a scope instance, parameter or environment that has ended |
| arena use after arena end | PROV-4, MEM-4, CLO-4 | an access reaches a value placed in an arena whose `arena.with` has returned |
| resource not finalised exactly once | DROP-2, DROP-4 | at completion, constructed resources differ from released plus forgotten plus returned |
| finaliser order not reverse declaration | DROP-2 | cleanup releases a declared owner before a live later declaration or descendant owner |
| allocation inside `@noalloc` | MEM-5 | a noalloc body constructs or deep-copies counted storage, or mutates a block that needs a detach |
| allocating call inside `@noalloc` | MEM-5 | a noalloc body calls a definition or implicitly invokes a destructor not declared `@noalloc` |
| freeze of a value another holder shares | MEM-11, MODE-2 | `shared.freeze` receives a value one of whose tracked blocks has another live holder |

An **access** is any operand extraction (read, copy, take, borrow, `mut`), the destination of
a write through a projection, and a drop of live storage, the final return to the harness
included.

Each value node carries a shadow set of the scope, parameter, borrowed-capture, environment
and arena instances that bound it. Plain bits have no bound, and inline payloads have
separate field bounds. Scope exit, frame exit, arena callback completion and a self-tail
restart end their instances. Copies keep local source bounds; borrowed non-global call and
capture lifetimes include the binding's home; a result relation substitutes the selected
inputs; a closure or function-type result `in env` keeps the callable's environment. A
global value lends no lasting bound. Native callback returns and write-backs are checked
after the callback frame exits. Named-owner release order follows declaration order and
scope ancestry across cleanup chains, independently of storage-liveness scheduling. Fresh
and taken blocks keep uniqueness witnesses; a semantic copy invalidates them and adds a
holder of the block, which lives while some shadow keeps it, and the storage a native `mut`
operation writes back keeps the holders it took. A copy of an element of a non-inline block
adds no holder, since such a block has one shadow. Arena root
builders stay writable, while a nested counted detach still allocates.

This is an oracle over executed paths, not a proof of unexecuted edges: a missing scope end
delays the audit to frame exit or restart. The gate requires zero checked traps and
divergences over the corpus and its agreement variants.

## The conformance corpus

The corpus under `conformance/` pins every rule. Programs are filed by rule prefix
(`conformance/OWN/`, `conformance/MEM/`, …) and named `RULE-N-description-kind.hir`. Each
starts with a comment header:

```text
// rule: OWN-3
// expect: reject OWN-3 at line 13
```

`rule` names the rule the program tests, which must resolve to exactly one definition in
this specification. `expect` is one of:

- `accept`: the program checks cleanly;
- `reject RULE at line N`: the first diagnostic cites `RULE` with its primary span on line
  `N` ([diagnostics](diagnostics.md));
- `run`: the program runs to completion, with optional `// stdout:` lines (each one literal
  output line) and an optional `// exit: N`;
- `fail RULE at line N`: execution stops with a language failure citing `RULE` at line `N`,
  with optional `// stdout:` lines.

`// entry: name` selects a test entry other than `main`. Every rejected program has an
accepted twin that differs only in the violation. Directives appear only in the leading
comment block. A program with a `// spec: #N` comment depends on an open specification
question; a program in `conformance/m2-runtime.txt` needs runtime features not every engine
supports yet. Both stay in the corpus and are excluded from the gate.

Every corpus file is in canonical format, and the passing list `conformance/passing.txt` is
a ratchet: a passing program never silently stops passing.

## Agreement

The source checker and the independent IR checker must agree: no program the source checker
accepts may have a lowering the IR checker rejects, and no phase may report an internal
error. The agreement campaign applies a fixed catalogue of single syntax edits to every
corpus program that is not expected to be rejected (passing modes, `mut` on bindings, mode,
brand and placement annotations, a second use of a binding, statements moved into or out of
loops and branches, swapped or repeated statements, operands replaced by another binding of
the same type, expressions wrapped in a called closure, stores into fields and elements, and
more) and replays every variant through the real driver. It is gated at zero flagged
variants. Every IR checker rule has a hand-built IR control that the rule is shown to
reject.

## Native code

Native code is compiled from validated typed IR through a low-level IR and LLVM, and is held
to the interpreter: identical outcome, output, effects and observation on every executed
corpus program and agreement variant, for every supported configuration
([DET-2](determinism.md#det-2-optimisation)). Targets are `x86_64-unknown-linux-gnu` and
`aarch64-unknown-linux-gnu`; other targets are compile-only until their parity is
executed. One `TargetSpec` fixes the triple, CPU features, endianness, pointer width,
alignments, C convention, object format and the layout and runtime versions.

**Calling convention.** The Hirsel cross-unit ABI uses C-compatible primitives: an explicit
context pointer, an optional caller allocation destination, an output pointer for every
non-unit result, then the declared parameters in order; the function returns a status
byte. Closure and iterator thunks also receive an environment pointer immediately after the
context. Small scalars and block references are passed by value, borrowed inline aggregates
by read-only address, `mut` parameters by exclusive address, and taken inline aggregates by
an address whose contents transfer at entry. The physical shape is computed once from the
specialized declared callable type, so changing the unit partition never changes a
published shape ([PROG-5](programs.md#prog-5-topology)). No attribute grants the backend
aliasing, non-null, alignment or dereferenceability facts beyond independently checked
lifetime and access evidence. C-facing callback thunks are monomorphic wrappers. `Ptr` and `FnPtr` use one
non-null pointer word. `Option[Ptr[T]]` and `Option[FnPtr[F]]` use that word's
null-pointer niche: zero is `None`, nonzero is `Some`. Their C ABI is one pointer,
including callback parameters/results and writable arguments. Both native backends
check non-null extern results (including discarded results), callback arguments and
pointer words written by C before admitting them to Hirsel. Zero is a PROG-4 foreign
contract violation that terminates the process at the extern call's site. Nested
extern calls restore the enclosing callback context and site when they return.
C-origin `FnPtr` values are opaque pass-back values; Hirsel cannot call them
([PROG-4](programs.md#prog-4-interop)).

### Aliasing facts

The native backends state an aliasing fact only where both checkers prove it. The fact
derives from the parameter's passing mode in the specialized `FnAbi`, never from a
callee's name, and the unit partition never changes it.

| Passing | What is proved | What native code states |
|---|---|---|
| `mut x` (an exclusive address) | The source checker gives the argument a write loan from callee entry to return, and every overlapping access conflicts with it ([OWN-3](ownership.md#own-3-exclusive-access); a scoped closure's captures count as arguments). The IR checker re-proves it at every call (`CallAlias`): no other `mut` argument overlaps the place, and no other argument or callee value holds a loan on an overlapping place, whether through its own views, borrowed record fields or a closure's borrowed captures. | LLVM `noalias` on the address. For the whole call, the place's bytes are reached only through pointers derived from it. |
| `mut x` where `x` is a Block or Box root of a private library body | The same | Nothing. The root travels by value as the block pointer the place holds, and that block may be shared. |
| `take x` (an inline aggregate) | The caller passes a fresh copy of the moved value | Nothing |
| `x` borrowed (an inline aggregate) | The caller passes a fresh read-only copy, and the IR checker rejects writes to borrowed storage | Nothing (no `readonly`). A drop wrapper passes the dying block's payload in place of a copy. |
| Borrowed or taken scalars and block references | Values | Nothing. A block may have other holders. |
| `unique` | A count fact ([MODE-2](ownership.md#mode-2-uniqueness)), not a loan | Only count tests and the detach postconditions that `llvm.assume` exposes |

`noalias` covers only the place's own bytes: a list's root word, or an inline record's
fields. It covers nothing reached through a pointer loaded from the place. Such a loaded
child pointer can be a block that other arguments also reach, because a counted block can
have several holders until a write detaches it.

Where the facts stop:

- **`Shared` interiors** are frozen and only ever borrowed
  ([MEM-11](memory.md#mem-11-shared)). They are never a `mut` place and carry no attribute.
- **Mutex interiors**: `mutex.with` moves the payload into a library local and lends `mut`
  of that local. Any other task's access happens before the acquire or after the release,
  which LLVM's `noalias` permits.
- **`unsafe ffi`**: extern declarations and `Ptr[T]` words carry no attribute. A
  callback context is the address of a Hirsel place that C holds for the whole extern call,
  and C may hand it to a callback nested inside an activation that still has it
  ([PROG-4](programs.md#prog-4-interop)). So a program that names any C callback states no
  `noalias` on any `mut` parameter.
- **Foreign callbacks**: the thunk passes C's context pointer straight to the entry, which is
  why the program-wide exception above applies.
- **Region and arena storage**: region references, arena tokens and caller destinations
  travel by value with no attribute. Region and arena records are reached through the run
  context and are never `mut` places.
- **Runtime primitives**: a `RuntimePrimitive` declaration states nothing, since its body
  is the runtime's.
- **Checked-execution shadows** exist only in the interpreter. They audit executed paths
  and are not evidence for any native attribute.

Cranelift has no parameter aliasing attribute, so it states none of these facts. Both
backends produce the same observations.

### Failure and cleanup

Failures propagate by explicit status through compiler-generated cleanup code backed by
immutable frame and cleanup descriptors; a `Result` is an ordinary value inside a successful
result, never the failure status. Every cross-unit call is assumed able to fail; a proved
nonfailing private helper may omit the test internally. Conditionally live locals and fields
carry explicit initialization masks. Each owned resource moves through
`Live → Finalizing → FieldsPending → Dead`: it enters `Finalizing` before its user drop
runs and can never enter it again. A failure abandons the finalizer's remaining body,
cleans its locals, releases the pending resource fields from high to low and continues
with sibling cleanup; cleanup status accumulates and never skips siblings. If ordinary
return cleanup fails, the extracted return value is cleaned too and the caller's result
stays dead. `mut` results write back on both successful results and language failure.

A replacement is one transaction: evaluate the right-hand value at its source point;
validate the destination projections, bounds, tag, writability and copy-on-write
preparation before extracting it (a failure here cleans the temporary and keeps both
destination and old value as they were); install the new content and move the old one to a
fresh displaced slot with no user callback or failing operation between the two stores;
then finalize the displaced value. If that finalizer fails, the destination still holds the
new value; a `mut` argument has no rollback ([OWN-3](ownership.md#own-3-exclusive-access)).

### Flat fields written in place

A byte list that is only created empty, reserved, pushed and frozen whole into the Bytes
field of an element that a flat list builder appends is written where that element will
lie. The list's creation opens the element at that field
(`FlatField Open`), its reservations and pushes write the field's bytes in place
(`FlatField Reserve`, `FlatField Push`), its freeze is a jump, and the element's
construction becomes `FlatClose`, which completes the element around the bytes already
there and counts it. The list has no block, its freeze copies nothing, and nothing
releases it. The field must follow only fixed fields, so its bytes lie at a fixed offset
while they are written. A builder holds at most one open element, beyond its used bytes,
and every path from the opening reaches the close with no other write of the builder.

The LIR validator does not trust the importer's proof of that region. It replays open
elements over slot versions: a reservation, a push and a close need the version reaching
them to be open at their field, and an opening needs one open at no field. Every other
write of the builder defines a version with no open element, so a write between the
opening and the close leaves the close without its element. The checked heap executor
tracks each builder's open element and refuses any other write of the builder while one
is open. Without this optimization, the byte lists are ordinary lists with the same meaning.

### Memory without interpretation

These memory paths of compiled code do no runtime interpretation; none changes a program's
meaning, TIR or LIR, and each falls back to the general path when its condition fails.

- A boxed nullary variant on the heap (`box Leaf()`, of a fixed-size variant without a
  finalizer whose tag is an ordinal or a niche) reads one immortal static block, named by
  the content of its layout and ordinal: it allocates nothing, and its release is a count
  test. It is never written: the box is shared, so a write copies first.
- An inline aggregate slot whose only write is one constant (a `Repeat` table such as a
  decimal-digit table) and which is only read is the private read-only data itself, named by
  its content (`hirsel.data.<hash>`), and is indexed in place, never copied to the stack.
- The deep copy of a block whose layout has no counted children is one allocation and one
  byte copy, with no visited map; a copy of a larger graph keeps its first visited blocks
  in place before using a map.
- A unique heap block reset for reuse runs its drop program through compiled glue, or
  nothing at all when every counted field has moved out, and is its own reuse token,
  reinstalled inline; a partly moved block released whole runs nothing either.
- A block is freed after the children its drop program releases, except at the release
  that ends the program (through the arms of a last variant): nothing of the block remains
  to run, so it is freed first (a tail release, the runtime's `STEP_RELEASE_LAST`, layout
  descriptor version 11). Compiled drop glue, one function per layout named by its
  `LayoutKey`, then releases a tail child of the same layout by looping, so a list releases
  in one native frame; the runtime's worklist frees in the same order. Finalizers, closes
  and failures keep the DROP-2 order: no step that runs anything is reordered, and free order
  is not observable.

### Time in the checked heap executor

The checked heap executor audits programs that hold a monotonic clock, its timer waits,
scope deadlines and a `proc.run` deadline ([CONC-3](concurrency.md#conc-3-cancellation)),
without a host clock. Its run follows the deterministic simulation's virtual time and must
observe what the interpreter does. A host may let any pending deadline pass while any
operation runs, so the executor also runs the program once per timer choice point (an
operation a task starts while a deadline is pending, or a run racing its own deadline) with
the earliest pending deadline fired there: the reading jumps to that deadline before the
operation proceeds, so the deadline cancels the operation, wakes a sleeper first or times
the run out. Each such run must pass the executor's terminal audit; its observation belongs
to another legal schedule and is not compared. One early firing per run bounds the
exploration, as one injected failure per run bounds fault injection. Readings are only the
virtual ones and the deadlines a run armed, so samples stay monotonic per clock. A host
process child is outside the executor's model.

**Runtime.** One runtime library provides counted allocation and release, region and
arena storage, hashing (SipHash-1-3 keyed from the run's hash seed), `Io` providers,
recording and replay, and the network and TLS providers. Compiled code reaches it only
through its versioned Hirsel-ABI exports. The in-memory layout of flat values is private and
is not the [canonical codec](serialization.md).

The offline observation viewer is `hirsel_driver::observation_view`. It reuses
`launch::parse_recording_prefix` and `journal::recover`; durable recovery preserves
an exact event-to-Prepared index for tooling. `hirsel view` renders normalized JSON
or a logical Perfetto trace without host admission. Replay enrichment uses the same
checked, bound interpreter host-replay path and `Execution::observable` as execution
comparison. It never opens live providers or invokes transport exporters. See
[Offline observation views](determinism.md#offline-observation-views) for evidence,
timing-sidecar, completeness and diagnostic format contracts.
