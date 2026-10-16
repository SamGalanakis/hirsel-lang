# Implementation

This part states what implementations must agree on and how agreement is checked: the
reference interpreter is the oracle, the conformance corpus pins every rule, an independent
IR checker re-proves what the source checker accepts, checked execution audits memory and
lifetime behaviour, and native code must match the interpreter observably. The choices
below the observable contract (data layout, calling convention, failure propagation) are
implementation design, not language rules; they may change without changing any program's
meaning. Every executable artifact an
implementation publishes is independently validated, in debug and release builds alike.

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
and taken blocks keep uniqueness witnesses; a semantic copy invalidates them. Arena root
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
lifetime and access evidence. C-facing exports are monomorphic wrappers.

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

**Runtime.** One runtime library provides counted allocation and release, region and
arena storage, hashing (SipHash-1-3 keyed from the run's hash seed), `Io` providers,
recording and replay, and the network and TLS providers. Compiled code reaches it only
through its versioned Hirsel-ABI exports. The in-memory layout of flat values is private and
is not the [canonical codec](serialization.md).
