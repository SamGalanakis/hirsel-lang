# hirsel-proof-export

The prover-neutral proof seam (rules PS-1..PS-16):

- obligation ids `(DefPath, RuleId, kind, ordinal)` computed from TIR alone
  (`obligations.rs`);
- the per-definition TIR digest (`hirsel.tir.digest.v1`) and the obligation
  checksum over it and the referenced signature fingerprints;
- the canonical `hirsel.tir.export.v3` text (`wire.rs` encodes every TIR,
  type-graph and signature type under the export context) and its importer,
  which re-derives every signature fingerprint (one derivation for every
  signature; an owner view must seal to its public signature), recomputes every
  digest, table and checksum, requires canonical form, and re-validates each
  definition with V.

`sidecar.rs` is the PS-12 evidence consumer: the
`hirsel.proof.sidecar.v1` record shape and `assess`, which decides for every
obligation of an export whether its record is current (revision and checksum),
replayable (a checker the build trusts, a deterministic budget) and proved, and
otherwise why it keeps its check (PS-13). The trust hook is an explicitly
trusted external verdict; nothing is replayed. The driver maps accepted
obligations to removal forms (`docs/spec/proof-seam.md`, PS-13).

Consumers read the text out of process; there is no plugin API. Interpreter and
runtime reports retain attribution separately from the defining operation.
`Export::failure_obligations` maps that operation's (definition, rule, site) to
candidate obligation ids.

Dependencies: `hirsel-codec`, `hirsel-diag`, `hirsel-span`, `hirsel-syntax`,
`hirsel-tir`, `hirsel-types`, `hirsel-validate`; never an implementation crate
or the driver. The driver collects the closure and wires `hirsel export --tir`.
