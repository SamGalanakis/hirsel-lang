# hirsel-type-analysis

Stage-selected analyses over `hirsel-types` and declaration snapshots:

- `cleanup`: destruction summaries and specialization by owned actuals.
- `logical`: target-independent logical sizing and resolved structure.
- `parametric`: MODE-2 parametric uniqueness and supplying arguments.
- `representation`: successive nominal representation expansion.
- `spelling`: source-shaped diagnostic type names and disambiguation.

Each module owns its public functions; consumers import them directly. The crate
never queries implementation bodies. Callbacks retain stage-owned declaration
lookup, authority evidence, operational failures and conservative fallback policy.

Stable semantic definitions, graph transport and basic property queries stay in
`hirsel-types`. Canonical CT spelling stays there because it is used in instance
keys and compile-time evaluation. Nominal proof memoization and cleanup-bound
normalization stay beside the basic property queries that use them.

The extraction preserves algorithms and moves the existing tests with them.
