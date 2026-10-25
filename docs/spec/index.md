# Hirsel

Hirsel is a systems-level, general-purpose language whose primary user is a coding
agent. It is procedural with value semantics: loops and local mutation, no references,
no methods. A program is a set of definitions, each checked against the signatures it
uses. Ordinary code is safe and fast by default; where layout or lifetimes matter, you
drop down a layer, locally, without changing the meaning of anything else. All
authority, including allocation placement and concurrency, arrives as parameters; the
one ambient permission is allocating on the heap. Every source of nondeterminism is a
capability.

This specification defines the language, its core library, its standard source library,
its run and recording formats, the diagnostic policy and the prover-neutral proof seam.
Builds, topology files and deployment and simulation tooling are built on top of it and
are not specified here; the provider contracts they rely on (runners, code stores and
stores) are.

## Fixed goals

These goals decide between designs. A rule that would break one of them is wrong.

1. **An agent can write correct code without global knowledge.** Every top-level
   definition has a complete signature, and checking a definition reads only its own
   body and the declared signatures it uses ([LANG-6](language.md#lang-6-signatures)).
   An edit to a body that keeps its signature rechecks that body alone.
2. **Safe by default, fast by default.** Ordinary code has no undefined behaviour, no
   data races and no use-after-free; it compiles to counted, region-placed, in-place
   updated values without annotations ([memory](memory.md)).
3. **Layers differ in cost, never in meaning.** Layer 1 (layout and lifetimes) and
   layer 2 (`Unsafe`) are opt-in and local. Using them in one definition never changes
   the meaning of code that does not.
4. **Authority is explicit.** Files, network, clocks, randomness, tasks, output,
   environment, arenas, runners and `Unsafe` are values passed as parameters
   ([capabilities](capabilities.md)).
5. **Deterministic.** Given the same capability inputs and run configuration, a program
   behaves identically, at every optimisation level and in every compilation topology
   ([determinism](determinism.md)).
6. **One meaning.** No flag, build mode, prover, optimiser or topology changes what the
   checker accepts or what a program means
   ([DET-3](determinism.md#det-3-no-switches)).
7. **Every diagnostic cites one rule.** The first diagnostic for a program is fixed
   by [the diagnostic policy](diagnostics.md); rule IDs follow the reading convention below.

## Layers

**Layer 0** is what an agent writes without thinking: value semantics, signature modes,
reference counting for long-lived values, compiler-chosen regions for scoped values, and
capabilities. No annotations beyond signatures; an analysis annotation
([PS-17](proof-seam.md#ps-17-analysis-annotations)) is optional and means nothing to the
checker.

**Layer 1** drops down for layout and lifetimes: `flat` types, `Packed` variants,
generational slots, explicit arenas and regions, `Shared[T]`, `Inline[T, n]`, `@soa`,
`@layout(c)`, `unique` in-place guarantees and `@noalloc`.

**Layer 2** is `Unsafe`: raw pointers, manual memory and foreign functions.

Cost reports (where layer-0 code counts, copies or keeps checks) are tooling.

## Contents

| Part | Rules |
|---|---|
| [Syntax](syntax.md): lexical grammar, statements and newlines, the full grammar, precedence | LANG-1, LANG-14 |
| [Language](language.md): evaluation, names, mutation, signatures, types, errors, tests, recursion, blocks, arguments, patterns, bindings | LANG-2 to LANG-13, LANG-15 |
| [Ownership and modes](ownership.md): places and loans, locality, uniqueness, once, sendable, passing modes, parts | MODE-1 to MODE-7, OWN-1 to OWN-8 |
| [Brands and placement](brands.md) | PROV-1 to PROV-5 |
| [Closures](closures.md), including declared state promises | CLO-1 to CLO-5 |
| [Generics](generics.md): bounds and dictionaries | GEN-1 to GEN-6 |
| [Serialization](serialization.md): schemas and the canonical codec | SER-1, SER-2 |
| [Memory](memory.md): counting, arenas, regions, layout, flat and packed data, slots | MEM-1 to MEM-14 |
| [Capabilities and authority](capabilities.md) | CAP-1 to CAP-6 |
| [Concurrency](concurrency.md) | CONC-1 to CONC-8 |
| [Runners](runners.md) (partly shipped): places functions run, entries, calls, outcomes, linked tapes, recording levels, provider seams | RUN-1 to RUN-14 |
| [Storage](storage.md) (partly shipped): stores over a directory, transactions, the output gate, typed tables, key encoding, schema history, the engine and its storage seam | STORE-1 to STORE-13 |
| [Failure and destruction](failure.md) | FAIL-1 to FAIL-4, DROP-1 to DROP-4 |
| [Determinism and replay](determinism.md): run configuration, recording, Blocked | DET-1 to DET-4 |
| [Everyday semantics](semantics.md): integers, floats, strings, iteration, maps | SEM-1 to SEM-5 |
| [Contracts and facts](contracts.md) | FACT-1 to FACT-3 |
| [Compile time](compile-time.md) | CT-1 to CT-5 |
| [Programs and packages](programs.md): names, visibility, entry, interop, topology, launch | PROG-1 to PROG-6 |
| [Core library](core-library.md): the `core@1` signature ledger and its contracts | |
| [Standard library](std-library.md): the `std@1` source package | |
| [Diagnostics](diagnostics.md): order, owners, causes and primary sites | |
| [Implementation](implementation.md): the reference interpreter, checked execution, the conformance corpus, native code | |
| [The proof seam](proof-seam.md): obligation ids, content hashes, the TIR export, pluggable analyses | PS-1 to PS-19 |

For complete runnable examples, start with the [language tour](../tour.md).
For task fan-out, actors, mutexes and scheduling tests, see the
[concurrent agent guide](../concurrent-agents.md).

## Reading this specification

A rule is a heading `RULE-N: Title` followed by its normative statement. Rule IDs are
stable and defined exactly once: a retired rule keeps its ID and is never reused. A
diagnostic cites exactly one rule ID; conformance tests are filed under the rule they test.

Complete definitions in `hirsel` code blocks parse under the grammar. Fragments written
inline, omitted surrounding declarations, ellipses and call-and-result arrows such as
`arena.seal(take a, take root) -> Region[T]` are schematic: they show a signature or a
shape, not source a program may contain. They do not license expression type arguments,
top-level `let`s or placeholder bodies. The [core library ledger](core-library.md)
supplies the concrete contracts.

"Must", "cannot" and "is rejected" state checker or runtime obligations. "Fails" means a
deterministic language failure ([FAIL-1](failure.md#fail-1-kinds)). An item marked
**Unspecified** is a contract this specification does not yet fix; an implementation may
not invent a language-visible answer for it.

## Glossary

These terms name distinct contracts; the linked rules define them.

| Term | Meaning |
|---|---|
| Definition | A named declaration checked against the signatures it uses ([LANG-6](language.md#lang-6-signatures)). |
| Passing mode | Borrow, `mut` or `take`: how a call accesses an argument ([OWN-1](ownership.md#own-1-borrow) to OWN-3). |
| Place; loan | Tracked storage or scope; a read, write, identity or bound dependency on it ([places and loans](ownership.md#places-and-loans)). |
| Relation | A lifetime and placement contract, written `T in S` ([MODE-1](ownership.md#mode-1-locality)). |
| Brand | An identity contract, written `T of p` ([PROV-1](brands.md#prov-1-brands)). |
| Capability; authority summary | A value granting access; the capabilities a value holds ([CAP-1](capabilities.md#cap-1-authority), [CAP-2](capabilities.md#cap-2-charged-at-use)). |
| Resource | A noncopyable value with defined destruction points ([DROP-2](failure.md#drop-2-resources)). |
| Error; failure | An expected `Error` value in a `Result`; a deterministic bug outcome ([LANG-8](language.md#lang-8-errors), [FAIL-1](failure.md#fail-1-kinds)). |
| Provider | A versioned capability contract selected by the run configuration ([providers](determinism.md#run-configuration)). |
| Run | An execution bound to a selected program revision, entry and validated configuration ([DET-1](determinism.md#det-1-contract)). |

## A first look

```hirsel
pub fn main() -> Result[(), String] {
  Ok(())
}
```

This root-free entry needs no grants. For programs with I/O, manifests and tests, see the
[runnable language tour](../tour.md). Rule-specific examples accompany
[ownership](ownership.md), [memory](memory.md) and [structured tasks](concurrency.md).
The transactional store and the planned runner interfaces have examples in
[storage](storage.md) and [runners](runners.md).

## Not in Hirsel

These are excluded by design. Compile-time declaration generation and callable results
are reserved ([compile time](compile-time.md#reserved-for-future-compile-time-features)),
not excluded; type-shape inspection is [CT-3](compile-time.md#ct-3-shapes), generated
function bodies are [CT-5](compile-time.md#ct-5-generation), and conditional lets are
[LANG-13](language.md#lang-13-patterns).

- Files as the unit of code; import statements; ambient I/O, globals and implicit
  contexts.
- User-visible references, lifetime variables, weak references and aliasable mutable
  cells; tracing garbage collection and cycle collection; implicit sharing of counted
  memory between tasks and run-time marking.
- Arithmetic operator variants (`+%`, `+|`), a conditional operator, optional
  labels, qualified variant names, overloading including per-variant overload sets, and a
  test sublanguage.
- Pipe operators, methods as chaining, an implicit `it`, and `using`-style field import.
- Textual macro expansion, type classes, implicit conversions (beyond LANG-8's `?` arm injection), exceptions, universal
  equality and literal defaulting beyond [LANG-2’s nonempty List rule](language.md#lang-2-prefix-law);
  compile-time reflection on function bodies or source text. No part of a signature is inferred from an implementation body.
- Local recursion, polymorphic recursion, closure-owned mutable state, capture of
  `let mut` names or borrowed parameters by escaping closures, borrowing counted data into
  child tasks, partially unique values (`aliased` fields), list handles, and facts over
  several record fields.
- Proof obligations or pluggable analyses that decide acceptance: prover strength and
  analysis findings never change what compiles
  ([PS-19](proof-seam.md#ps-19-analyses-never-decide)).
- Fast-math, relaxed SIMD, build-mode or topology-dependent semantics, fallible implicit
  destructors, recoverable mutex poisoning, per-task heap budgets, and manual freeing of
  ordinary values outside `Unsafe`.

## Deterministic depth revision

The depth semantics identity is `hirsel.depth.invocations.v2` (2026-10-25). It specifies
[LANG-10](language.md#lang-10-recursion)'s 1,024 foreground invocations, which leaf
operations do not add to,
[FAIL-3](failure.md#fail-3-what-runs)'s independent 64 cleanup invocations and
[MEM-3](memory.md#mem-3-freeing)'s 1,088 logical structural edges, with the root at zero.
This revision replaces the unfinished frame-byte promise and visited-only structural
depth controls. Artifact validation and recording identities bind this semantic revision;
artifacts and recordings made under the former rule, or without the revision, are refused
with an explicit incompatible depth-semantics diagnostic. They are never reinterpreted.
There is no rollout flag. Bounded-arena charges remain independently normative under
[MEM-6](memory.md#mem-6-bounded-arenas).

Only the language activation and logical structural-depth rules produce comparable
Depth outcomes. Evaluator guards, proof/checker traversal limits, host resource budgets,
physical-stack guards and scratch-allocation failures do not become Depth because an
implementation could not complete an operation. An incomplete engine cell is not a
language failure, successful agreement, or evidence that the source is invalid.

## Unspecified

These contracts are deliberately open. Implementations report them as unresolved rather
than supply a language-visible answer.

- The optimizer-independent compile-time step budget and its charges
  ([CT-4](compile-time.md#ct-4-order)).
- Constructor-tail loop eligibility ([LANG-10](language.md#lang-10-recursion)).

The [coverage ledger](../../conformance/coverage-ledger.md) distinguishes these
open decisions from missing instrumentation, current defects and resolved feature
history. It records finite executable evidence without assigning normative budgets.

## Prior art

The design draws on value semantics, modes, reference counting, capability-based I/O,
structured concurrency and content-addressed compilation.
