# hirsel-intern

Process-global session identities, deterministic fan-out and work counters.

Allowed dependencies: std only.

Invariants and boundary:

- `Symbol(NonZeroU32)` comes from one append-only, process-global spelling
  interner (LANG-4). Equal spellings yield equal symbols for the life of the
  process; `Symbol::intern` and `Symbol::as_str` need no interner handle, and
  `as_str` returns the leaked `&'static str` without taking a lock.
- Eq and Hash compare ids; Ord and Debug use the spelling, so ordered
  collections and printed values never depend on intern order. Ids never enter
  persistent identity, hash encodings or output.
- No source-normalization or name-resolution policy is hidden here.
- `AppendTable` is the lock-free, never-moving row store behind every
  process-global id table (Symbol here, `DefId` in hirsel-syntax). `IdHasher`
  (FxHash) hashes dense ids for `DefMap` and the spelling index.
- `Scramble` is how every id kind (Symbol, DefId, and later TypeId and Anchor)
  turns a dense table position into an id. Under the `id-salt` feature,
  `salt_ids(seed)` multiplies those positions by an odd per-kind constant, which
  moves hash order and relative id order; seed 0 is the unsalted assignment.
  It must run before any id is assigned. `under_salts` reruns one test in a
  child process per seed and returns each run's output, for the O1 invariance
  tests.
- `persist` lets a persisted form carry its own id tables (the std image): inside
  an encoding scope an id serializes as its number in a `Numbering`, assigned in
  first-encounter order, and inside a decoding scope a number reads back
  through a `Source`. Symbols use `with_symbol_numbering`/`with_symbol_source`;
  hirsel-syntax keeps DefId's scope stack. Outside a scope a Symbol is its
  spelling. `with_symbol_spellings` masks an enclosing table while a nested archive persists its own complete names.

`jobs::map(jobs, tasks, cost, work)` is the one fan-out path. Tasks start
longest first by `cost`, ties by input index; results come back in input order.
One worker (or one task) runs on the caller's thread; otherwise workers are
scoped threads with `COMPILER_STACK_BYTES` stacks, and each worker's work
counters are added to the caller's when it finishes. After a panic no new task
starts, and the earliest-started panicking task's payload is resumed on the caller.

`work` is the shared work registry: the lowest crate every phase reaches, so one
`Counter` enum plus `count`/`count_by`/`read` serves all per-thread deterministic
work measurements instead of per-crate thread-local counters.

`OwnershipFlowSteps` and `OwnershipValueJoins` measure ownership solver transfers
and value joins, and `OwnershipStateRows` the temporaries each transfer carries,
for machine-independent scaling gates.
They are instrumentation only; counter ordinals are not semantic encodings.

`RcCleanupVersionRows` counts instruction and terminator rows transferred while
solving and replaying cleanup version inputs, for the driver's native scaling probes.

`timings` holds the compile timing points of docs/building.md, "Compile timings":
`interval` guards for the driver's additive wall intervals, `phase` guards for
per-phase work and CPU summed over threads, `wait` and `count`. Totals are
process-wide atomics; when timings are off each guard is one relaxed load.
`memory` records the heap bytes a binary's installed heap meter attributes to a
load (`retained`) or to a dropped table (`released`) for `--mem-usage`.
