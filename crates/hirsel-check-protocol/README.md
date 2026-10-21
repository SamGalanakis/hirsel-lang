# hirsel-check-protocol

Shared phase decisions and declaration conventions for signature normalization,
body checking, source auditing and TIR lowering. This small data crate keeps
hirsel-signature, hirsel-check and hirsel-lower independent compilation units.

Allowed dependencies: hirsel-span, hirsel-intern, hirsel-syntax and hirsel-types. No phase,
driver, interpreter or validator dependency, including in tests.

- `body` owns the existing H-produced plans: checked propagation, implicit loop
  calls and execution, function results, shape arguments, contract clauses,
  literal patterns, resource patterns and distinct facts. H constructs them;
  I/J enrich the checked tables; K transports those decisions without inferring
  types or modes. Every TypeId is a row of the session `TypeInterner`.
- `declaration` owns the five pure rules reused by header normalization and
  body annotations: part relations, named-result relations, environment-only
  result candidates and builtin argument bounds. They inspect written syntax
  or declared builtin identity, never bodies or provider state.
- `header` owns one written header's normalization request (`Input`) and its
  result (`NormalizedDeclaration`, with the owner's private
  `OwnerRepresentation`). hirsel-signature produces it; hirsel-engine consumes
  it through its `Phases` trait object without depending on the signature crate.
- `fact` owns private predicate paths and the callable convention derived from
  the declaring signature. It supplies no executable implementation.

These records and helpers moved intact from hirsel-check. Import their actual
module directly; there are no exports through the implementation crates. The
frozen syntax/types/TIR public types and semantic encodings are unchanged.

ForProtocol::Bytes selects the existing indexed execution plan with a checked bytes.len call and a plain u8 index result. It carries no new TIR operation.
