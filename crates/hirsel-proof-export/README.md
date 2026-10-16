# hirsel-proof-export

The prover-neutral proof seam (rules PS-1..PS-16):

- obligation ids `(DefPath, RuleId, kind, ordinal)` computed from TIR alone
  (`obligations.rs`);
- the per-definition TIR digest (`hirsel.tir.digest.v1`) and the obligation
  checksum over it and the referenced signature fingerprints;
- the canonical `hirsel.tir.export.v1` text (`wire.rs` encodes every TIR,
  type-graph and signature type under the export context) and its importer,
  which re-derives every signature fingerprint (one derivation for every
  signature; an owner view must seal to its public signature), recomputes every
  digest, table and checksum, requires canonical form, and re-validates each
  definition with V.

Consumers read the text out of process; there is no plugin API. Interpreter and
runtime `Failure` records are unchanged: `Export::failure_obligations` maps a
failure's (definition, rule, site) to candidate obligation ids.

Dependencies: `hirsel-codec`, `hirsel-diag`, `hirsel-span`, `hirsel-syntax`,
`hirsel-tir`, `hirsel-types`, `hirsel-validate`; never an implementation crate
or the driver. The driver collects the closure and wires `hirsel export --tir`.
