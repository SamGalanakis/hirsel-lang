# External oracles

- **CAP-1, confinement soundness:** a finite program can witness an escape but cannot prove the absence of every escape. Run directory-capability sandbox fuzzing over relative paths, symlinks and concurrent filesystem changes; audit capability grants and projections. Concrete relative-path escape cases are pending the library surface, separately recorded in COVERAGE.md.
- **CAP-2, purity:** repeat functions with inert signatures over identical inputs in the interpreter and compiled code, under different capability implementations and allocator states. Their results and failures must agree. Use a checker model to verify recursive authority summaries, opaque summaries and drop authority; the corpus has finite summary witnesses.
- **CAP-3, restriction soundness:** a checker/authority model must prove that scoped grants, uses restrictions and inert results compose without leakage. The corpus tests individual capture restrictions.
- **CAP-4, scheduling attribution:** deterministic Io event logs must include every atomic and mutex acquisition. Replay the same decisions and compare outcomes; a single program cannot establish that the runtime never bypasses Io.
