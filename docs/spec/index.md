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
Code storage, builds, topology files, deployment and simulation tooling are built on top
of it and are not specified here.

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
   environment, arenas and `Unsafe` are values passed as parameters
   ([capabilities](capabilities.md)).
5. **Deterministic.** Given the same capability inputs and run configuration, a program
   behaves identically, at every optimisation level and in every compilation topology
   ([determinism](determinism.md)).
6. **One meaning.** No flag, build mode, prover, optimiser or topology changes what the
   checker accepts or what a program means
   ([DET-3](determinism.md#det-3-no-switches)).
7. **Every diagnostic cites one rule.** Rule IDs such as `OWN-6` are stable and never
   reused; each is defined exactly once in this specification, and the first diagnostic
   for a program is fixed by [the diagnostic policy](diagnostics.md).

## Layers

**Layer 0** is what an agent writes without thinking: value semantics, signature modes,
reference counting for long-lived values, compiler-chosen regions for scoped values, and
capabilities. No annotations beyond signatures.

**Layer 1** drops down for layout and lifetimes: `flat` types, `Packed` variants,
generational slots, explicit arenas and regions, `Shared[T]`, `Inline[T, n]`, `@soa`,
`@layout(c)`, `unique` in-place guarantees and `@noalloc`.

**Layer 2** is `Unsafe`: raw pointers, manual memory and foreign functions.

Using a lower layer in one type or definition never changes the meaning of code that does
not use it. Cost reports (where layer-0 code counts, copies or keeps checks) are tooling.

## Contents

| Part | Rules |
|---|---|
| [Syntax](syntax.md): lexical grammar, statements and newlines, the full grammar, precedence | LANG-1, LANG-14 |
| [Language](language.md): evaluation, names, mutation, signatures, types, errors, tests, recursion, blocks, arguments, patterns, bindings | LANG-2 to LANG-13, LANG-15 |
| [Ownership and modes](ownership.md): places and loans, locality, uniqueness, once, sendable, passing modes, parts | MODE-1 to MODE-7, OWN-1 to OWN-8 |
| [Brands and placement](brands.md) | PROV-1 to PROV-5 |
| [Closures](closures.md), including declared state promises | CLO-1 to CLO-5 |
| [Generics](generics.md): bounds and dictionaries | GEN-1 to GEN-5 |
| [Serialization](serialization.md): schemas and the canonical codec | SER-1, SER-2 |
| [Memory](memory.md): counting, arenas, regions, layout, flat and packed data, slots | MEM-1 to MEM-14 |
| [Capabilities and authority](capabilities.md) | CAP-1 to CAP-5 |
| [Concurrency](concurrency.md) | CONC-1 to CONC-8 |
| [Failure and destruction](failure.md) | FAIL-1 to FAIL-4, DROP-1 to DROP-4 |
| [Determinism and replay](determinism.md): run configuration, recording, Blocked | DET-1 to DET-4 |
| [Everyday semantics](semantics.md): integers, floats, strings, iteration, maps | SEM-1 to SEM-5 |
| [Contracts and facts](contracts.md) | FACT-1 to FACT-3 |
| [Compile time](compile-time.md) | CT-1 to CT-4 |
| [Programs and packages](programs.md): names, visibility, entry, interop, topology, launch | PROG-1 to PROG-6 |
| [Core library](core-library.md): the `core@1` signature ledger and its contracts | |
| [Standard library](std-library.md): the `std@1` source package | |
| [Diagnostics](diagnostics.md): order, owners, causes and primary sites | |
| [Implementation](implementation.md): the reference interpreter, checked execution, the conformance corpus, native code | |
| [The proof seam](proof-seam.md): obligation ids, content hashes, the TIR export | PS-1 to PS-16 |

For complete runnable examples, start with the [language tour](../tour.md).

## Reading this specification

A rule is a heading `RULE-N: Title` followed by its normative statement. Rule IDs are
stable: a retired rule keeps its ID and is never reused. A diagnostic cites exactly one
rule ID; conformance tests are filed under the rule they test.

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

## A first look

A program selects the standard library explicitly. Save the example below as `main.hir`
and select it with this manifest, whose dependency path is relative to the manifest
([packages](programs.md#package-selection)):

```json
{"schema":1,"package":{"name":"application","version":"1"},"entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":"stdlib/hirsel.json","package":{"name":"std","version":"1"},"expose":[]}]}
```

```hirsel
pub fn main(rt: Io, args: Args) -> Result[(), CliError] {
  let root = arg.path(args, 0) ? Usage
  let dir = io.dir(rt, root) ? Files
  let n = normalize_tree(dir) ? Files
  out.print(io.stdout(rt), u64.to_string(n)) ? Output
  Ok(())
}

pub fn normalize_tree(dir: Dir) -> Result[u64, FileError] {
  let mut changed: u64 = 0
  let walk = file.walk(dir)?              // opening may fail
  for step in take walk {                // items are Result[Path, FileError]
    let path = step?
    let old = file.read_bytes(dir, path)?   // owned result; locality follows dir at this call
    let new = bytes.replace(old, b"\r\n", b"\n")
    if new != old {
      file.write_bytes(dir, path, take new)?
      changed += 1
    }
  }
  Ok(changed)
}

pub fn test_normalize(t: Test) {
  let dir = test.dir(t, [(path.of("a.txt"), b"x\r\ny")])
  test.check(t, normalize_tree(dir) == Ok(1))
}

// library transaction: fn transact[R: inert, E: inert](db: Db,
//   body: fn(tx: Tx) -> Result[R, E] uses {}) -> Result[R, TxFailure[E]]
// Tx is a scoped capability (MODE-6): it cannot leave the body
pub fn reserve(db: Db, sku: Sku, n: u32)
  -> Result[Reservation, TxFailure[StockError]] {
  store.transact(db, fn(tx: Tx) {
    let have = stock.get(tx, sku)?
    if have < n { return Error(OutOfStock(sku)) }
    stock.put(tx, sku, have - n)?
    Ok(reservations.insert(tx, Reservation(sku: sku, count: n))?)
  })
}

// structured tasks: children capture sendable values (Dir, Path are plain)
pub fn sizes(tasks: Tasks, dir: Dir, a: Path, b: Path)
  -> Result[(u64, u64), FileError] {
  task.scope(tasks, fn(s: Scope) {
    let ha = task.async(s, fn() { file.size(dir, a) })   // Task[..] of s
    let hb = task.async(s, fn() { file.size(dir, b) })
    Ok((task.await(s, take ha)?, task.await(s, take hb)?))
  })
}

// a getter: the result is no longer-lived than m; with a heap map
// at the call site it is an ordinary heap value
pub fn get[K: key, V](m: Map[K, V], k: K) -> Option[V] in m
```

Layer 1 and declarations:

```hirsel
@noalloc
pub fn saxpy[n: size](a: f32, x: Inline[f32, n],
                      mut y: Inline[f32, n]) {
  for i in 0..n { y[i] = a * x[i] + y[i] }   // no checks: lengths equal by type
}

@incomplete
pub fn parse(arena: Arena, src: Bytes)
  -> Result[Ast, ParseError] in arena { todo }

// layer 1: flat data is one contiguous block; serialization is a separate opt-in
flat type Item { sku: String, qty: u32 }
flat type Order { id: u64, customer: String, items: List[Item] }

// layer 1: generational ids for lifetimes that do not nest
fn remember(mut nodes: Slots[Node], take node: Node) -> Option[Node] in nodes {
  let id = slots.insert(mut nodes, take node)      // SlotId[Node] of nodes
  slots.get(nodes, id)                             // Option: None once removed
}

// layer 1: read-only data many tasks share; thaw when done
fn publish(take built: unique Index) -> Shared[Index] {
  shared.freeze(take built)
}

resource type AtomicFile { dir: Dir, tmp: Path, dst: Path }
  drop(f) { let _ = file.remove(f.dir, f.tmp) }

pub fn save(dir: Dir, p: Path, data: Bytes)
  -> Result[(), FileError] {
  let mut f = file.create_atomic(dir, p)?   // AtomicFile of dir
  file.write_all(dir, mut f, data)?            // data borrowed for the call
  file.commit(dir, take f)                 // rename; fallible
}

pub fn at[T](xs: Array[T], i: u64) -> T in xs
  requires i < array.len(xs)
pub fn append[T](mut xs: List[T], take x: T)
  ensures list.len(xs) == before(list.len(xs)) + 1

distinct type Percent(v: u8) where v <= 100   // plain, data, key
@layout(c) type Header { magic: u32, len: u16, flags: u16 }
type Tree = Leaf | Node(left: box Tree, key: u64, right: box Tree)
type Log { dir: Dir, file: File of .dir }
```

The `reserve` example uses signature-only application definitions listed in
[the core library](core-library.md#example-application-definitions); they are fixtures,
not ambient database authority.

## Not in Hirsel

These are excluded by design. Future typed compile-time generation, type-shape
reflection and callable evaluation are reserved ([compile time](compile-time.md#reserved-for-future-compile-time-features)),
not excluded.

- Files as the unit of code; import statements; ambient I/O, globals and implicit
  contexts.
- User-visible references, lifetime variables, weak references and aliasable mutable
  cells; tracing garbage collection and cycle collection; implicit sharing of counted
  memory between tasks and run-time marking.
- Arithmetic operator variants (`+%`, `+|`), a conditional operator, `if let`, optional
  labels, qualified variant names, overloading including per-variant overload sets, and a
  test sublanguage.
- Pipe operators, methods as chaining, an implicit `it`, and `using`-style field import.
- Textual macro expansion, type classes, implicit conversions, exceptions, universal
  equality and literal defaulting; compile-time reflection on source definitions and
  generation of declarations or bodies. No part of a signature is inferred from an
  implementation body.
- Local recursion, polymorphic recursion, closure-owned mutable state, capture of
  `let mut` names or borrowed parameters by escaping closures, borrowing counted data into
  child tasks, partially unique values (`aliased` fields), list handles, and facts over
  several record fields.
- Proof obligations that decide acceptance: prover strength never changes what compiles.
- Fast-math, relaxed SIMD, build-mode or topology-dependent semantics, fallible implicit
  destructors, recoverable mutex poisoning, per-task heap budgets, and manual freeing of
  ordinary values outside `Unsafe`.

## Unspecified

These contracts are deliberately open. Implementations report them as unresolved rather
than supply a language-visible answer.

- The logical frame-byte table behind `Depth` ([LANG-10](language.md#lang-10-recursion))
  and the structural scratch levels ([MEM-3](memory.md#mem-3-freeing)).
- The logical size table charged by bounded arenas
  ([MEM-6](memory.md#mem-6-bounded-arenas)) and the frame charges of arenas and scratch.
- The optimizer-independent compile-time step budget and its charges
  ([CT-4](compile-time.md#ct-4-order)).
- Constructor-tail loop eligibility ([LANG-10](language.md#lang-10-recursion)).
- Complete layout composition rules for `@soa`, variants and flat types, and the native
  data ABI ([MEM-7](memory.md#mem-7-layout)).
- The complete foreign-function ABI: `unsafe ffi` package metadata, wrapper kind and
  model declarations, `FnPtr` callbacks, exports and retention
  ([PROG-4](programs.md#prog-4-interop)).

## Prior art

Hirsel adapts ideas from OxCaml (modes, locality, uniqueness, once, portability), Koka's
Perceus and Lean 4 (reference counting, reuse, freeing worklists), Goose (data stacks,
segments that never move, flat types, variable-size variants, share-nothing tasks),
Gleam (visibility, labelled arguments), Swift and Hylo (value semantics), Mojo (origins
by argument name, `mut`), Zig (`Io`, destruction, cancellation), Rust (contracts, `Arc` as
`Shared`, scoped tasks), Ada and C++26 (contracts, `'Old`), Kotlin (inline closure
specialisation), Flix and Effekt (authority at use), Wasm and WASI (handles, directory
capabilities, float determinism), Trio (cancel scopes, nurseries), Jai (compile-time
execution), MLton and Swift whole-module optimisation, Unison and rustc (content-addressed,
query-based incrementality), and Vx (rule-linked conformance).
