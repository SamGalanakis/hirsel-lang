# hirsel-intern

Intern repeated spellings into a compact symbol arena.

Allowed dependencies: std only.

Invariants and boundary:

- Symbol is a u32 index into exactly one Interner (LANG-4).
- Equal spellings yield equal symbols in that interner.
- Intern order is not semantic and symbols never enter persistent identity/hash encodings.
- No source-normalization or name-resolution policy is hidden here.

SyntaxFile now retains an Arc<Interner> snapshot cloned after lexing. Interner
supports Eq/PartialEq for snapshot data; equality does not establish cross-session
symbol identity. New spellings in the session interner do not mutate a published
snapshot. All emitted keyword spellings are interned by the lexer for members.

Interner clones share immutable spelling rows. Interning an existing spelling does
not detach them; a new spelling detaches only when a published snapshot still
shares the rows. `work::read(Counter::CopiedSymbols)` counts actual spelling rows
copied on this thread for deterministic query-scaling tests. Snapshot equality and
Debug retain the previous value semantics and strings/indices shape.

`work` is the shared work registry: the lowest crate every phase reaches, so one
`Counter` enum plus `count`/`count_by`/`read` serves all per-thread deterministic
work measurements instead of per-crate thread-local counters.

`OwnershipFlowSteps` and `OwnershipValueJoins` measure ownership solver transfers
and value joins for machine-independent scaling gates.
They are instrumentation only; counter ordinals are not semantic encodings.

`RcCleanupVersionRows` counts instruction and terminator rows transferred while
solving and replaying cleanup version inputs, for the driver's native scaling probes.
