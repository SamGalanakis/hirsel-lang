# hirsel-proof-export

The prover-neutral proof seam (rules PS-1..PS-16):

- obligation ids `(DefPath, RuleId, kind, ordinal)` computed from TIR alone
  (`obligations.rs`);
- the per-definition TIR digest (`hirsel.tir.digest.v1`) and the obligation
  checksum over it and the referenced signature fingerprints;
- the canonical `hirsel.tir.export.v6` text (`wire.rs` encodes every TIR,
  type-graph and signature type under the export context, including a recursive
  type group member's `recursive_group` key) and its importer,
  which re-derives every signature fingerprint (one derivation for every
  signature; an owner view must seal to its public signature), recomputes every
  digest, table and checksum, requires canonical form, and re-validates each
  definition with V.

`content.rs` computes LANG-6 definition hashes: the same wire text in content
mode (`Cx::content`), where every definition path is written as its hash
(through the codec's `Observe::reference`), a reference into the recursive
group being hashed as a group marker, and spans, local, parameter and
generic-parameter names and name-derived fingerprints are erased. A
signature's nominal roles are written by role name, so an edit to a core role
type moves only the definitions whose TIR or types name it.
`definition_groups` hashes Tarjan components dependencies first and returns
each group's members (ordered by hash, then text) with their content texts,
group references and hashes; `definition_hashes` flattens them.
`group_hashes` is the one hash computation, from texts and group references
alone: it refines each member until the group's partition is stable, so a code
store recomputes a stored group's hashes with it. `content_references` reads
the `(Ref ...)` nodes of a content text (definition hashes and group markers).
`call_sites` lists a TIR's calls and function references with each callee's
hash. The driver-layer engine chooses each definition's `DefinitionContent`.

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
