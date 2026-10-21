# The proof seam

Hirsel builds no prover. It publishes three prover-neutral primitives that any consumer (a
Lean projection, an SMT or Why3 verification-condition generator, a Dafny- or Verus-style
tool, an AI proof agent) builds on out of process: stable **obligation ids**, **content
hashes** that say when evidence is stale, and a canonical **TIR export** that the importer
re-validates. The same seam carries **pluggable analyses** (taint, units of measure,
typestate, cost bounds, termination and the like): source annotations no checker reads, the
export that carries them, and a findings sidecar for the results
([PS-17](#ps-17-analysis-annotations) to [PS-19](#ps-19-analyses-never-decide)). This part
fixes their shape and the rules every consumer obeys. Rule IDs are `PS-n`.

Nothing here changes what compiles or what a program means
([not in Hirsel](index.md#not-in-hirsel), [FACT-3](contracts.md#fact-3-checking),
[DET-3](determinism.md#det-3-no-switches)). The export is read-only with respect to the
compiler: lowering, the low-level IR, interpreter failure records and every other digest
are unchanged by it.

## Obligation identity

### PS-1: Id

`ObligationId = (DefPath, RuleId, kind, ordinal)`, written `pkg@ver/mod/name#RULE/kind/ordinal`,
for example `standalone@0/clamp#FACT-1/ensures/0`. The ordinal is the position among the
definition's sites with the same `(RuleId, kind)` under the canonical traversal
([PS-3](#ps-3-canonical-traversal)). Ids never contain arena indices, spans or hashes.

### PS-2: Kinds

Kinds are a closed, versioned list. Later clause forms (`invariant`, `decreases`,
`fails_if`, …) arrive as new kinds; existing kinds and their ordinals never change meaning,
so adding a kind is not a format break ([PS-12](#ps-12-sidecar-evidence-only)).

| Kind | Site in TIR | Rule | Ordinal over |
|---|---|---|---|
| `check` | an `InstructionKind::Check { rule }` that is not a contract site (FACT-2 constructor facts, entry checks, contract checks of closures) | the check's rule | check sites |
| `failure-edge` | an operation that can take its failure edge: an `Assign` of integer `Add`, `Sub`, `Mul`, `Div`, `Rem`, `Shl`, `Shr` or integer `Neg` (SEM-1); a projected place with an `Index` (MEM-14) or a `VariantField` (MEM-13) projection, one obligation per (body, root, projection) whose sites are every instruction or terminator naming it; a `Terminator::Fail { rule }` | SEM-1, MEM-14, MEM-13, or the `Fail`'s rule | sites; a projected place at its first naming |
| `requires-at-call` | a `Terminator::Call` whose callee's function type (the signature for `Callee::Definition`, the operand's type for `Callee::Indirect`) has `requires` clauses: one obligation per (call, clause), clauses in declaration order | FACT-1 | (call, clause) pairs |
| `ensures` | one per `ensures` clause of the definition; its sites are the exit checks on every return path | FACT-1 | clauses, declaration order |
| `requires` | one per `requires` clause of the definition, the precondition as an assumption; its site is the entry check | FACT-1 | clauses, declaration order |

A contract site is a `Check { rule: FACT-1 }` whose condition is the result of an
`Indirect` call of a closure over a body listed in `Definition.contracts`, created in the
entry body. Its clause is the `Definition.contracts` ordinal, and its kind comes from the
owner signature's `Contract` with that ordinal. Library operation failures belong to the
callee, call failure edges to the callee's obligations, `Unreachable` to the IR validator
(it is a compiler assertion, not a user obligation), and MEM-3 depth failures to resource
limits ([PS-11](#ps-11-proof-never-decides)): none of these produce obligations.

### PS-3: Canonical traversal

Bodies in `Definition.bodies` order, blocks in arena order, instructions in order, then the
terminator; at one instruction, its own `check` or arithmetic site first, then the places it
names in field order; at a terminator, its `Fail` or call sites, then its places. Lowering
appends in source order, so this is source order within each body. An obligation table lists
the `requires` obligations first, then the sites in this order, then `ensures`.

### PS-4: Stability

An obligation's id is unchanged by:

- any edit to another definition, including its body, its signature, or adding or removing
  definitions anywhere;
- an edit to this definition that adds, removes or moves no site of the same
  `(RuleId, kind)` earlier in PS-3 order (comments, spacing, renames, code of other kinds,
  sites later in the order).

Its ordinal shifts when a same-`(RuleId, kind)` site is added or removed earlier in the
order, when code containing such sites is reordered, or when the lowering of this definition
changes. Ids are positional, not semantic: a consumer that wants to carry evidence across
such edits re-pairs by checksum ([PS-6](#ps-6-obligation-checksum)) or by its own shape
matching, never silently. `requires-at-call` ordinals also shift when the callee gains or
loses a `requires` clause, because the callee's signature changed.

## Content hashes

### PS-5: TIR digest

Per definition, the digest is `H("hirsel.tir.digest.v1", TIR_VERSION, wire text of the
definition's (Definition …) node)`, where `H` is length-framed SHA-256. The wire text
([PS-8](#ps-8-encoding)) excludes sessions: spans are location-table indices, syntax-arena
ids are renumbered per definition, and type ids are indices into the definition's own type
table. The location table itself is not hashed, so moving the definition in its file, or
editing anything before it, leaves the digest unchanged. The domain changes only with
`TIR_VERSION` or a change to the TIR wire form.

### PS-6: Obligation checksum

`H("hirsel.obligation.checksum.v1", id text, TIR digest, (DefPath, signature fingerprint)…)`,
over every definition the definition's TIR names, itself included (callees, function
references, constant reads, constructed and matched nominals, named types and capabilities
in its type graph, origins, protocol association sources), sorted by `DefPath` and
deduplicated. The export lists these references with their fingerprints. Signature
fingerprints already include their own dependencies' fingerprints; a member of a recursive
type group (type declarations naming each other through their bodies) lists no other
member and instead includes the group's key, which every member's text moves. By
[LANG-6](language.md#lang-6-signatures) a callee body edit changes neither the caller's TIR
nor any fingerprint the caller references, so it never changes a caller's checksum; a
callee signature edit changes its fingerprint and therefore every checksum of every
referencing definition. The checksum is per definition plus id; a finer per-obligation slice
may later be added as a new checksum domain beside this one.

### PS-7: What digests bind

Evidence is current for an obligation exactly when its recorded `(ObligationId, checksum)`
equals the export's. The export header names the `TIR_VERSION`, compiler version, spec hash
and interface version that produced it; a consumer that pins a checker also records them.

## The export: `hirsel.tir.export.v7`

### PS-8: Encoding

Version 6 carries TIR version 6 (size-generic constant signatures, constant-read size
actuals, order-bearing empty SortedMap constructors, caller-attribution declaration
metadata, the explicit tracked body set and indirect-call boundaries) and adds the analysis
annotations section ([PS-17](#ps-17-analysis-annotations)). Version 7 writes each
definition, signature and materialized value with its canonical type table, the closure of
the type ids it names in first-encounter order, in place of its producer's type store; TIR
digests change accordingly. Imports reject earlier formats.

The export is one canonical text in the S-expression form of the `hirsel-codec` crate: one
spelling per value, so decode-then-encode is byte-identical and two runs of one build on one
input give identical bytes ([DET-1](determinism.md#det-1-contract)). No debug rendering, map
iteration order, file id, type-id provenance, address or host path appears.

- **References by name.** Cross-definition references are `DefPath`s. Arena ids appear only
  inside the artifact that owns the arena (body-local locals and blocks, type-table indices,
  location-table indices).
- **Syntax ids.** Expression, pattern, type-expression, statement and item ids are
  file-arena indices, renumbered per definition by order-preserving rank, so sorted sets stay
  sorted and the numbering is stable under edits elsewhere in the file.
- **Interned tables.** Each definition has its own type table and location table, so every
  definition section is self-contained. A location is `(module, line, column, end line, end
  column)`, one-based, columns in Unicode scalars; `module` is the source file's module
  identity (`std@1/net`), never a path. Locations are deduplicated by value and numbered by
  first occurrence in encoding order. Decoded spans are location handles: `file` is the
  definition section's ordinal and `start == end` indexes its table.

### PS-9: Contents

- **Header:** the format `hirsel.tir.export.v7`, `TIR_VERSION`, compiler version, spec hash,
  interface version, and the launch entry with its semantic program revision when the
  program declares one.
- **Definitions,** sorted by `DefPath`: path, location table, TIR, TIR digest, references
  with fingerprints, operations used (callees with a signature in the export and no body,
  exported or skipped: registry operations, whose signatures carry effects and authority),
  and the obligation table: PS-1 id, checksum, every TIR site as
  `(body, block, instruction | terminator, location)` with the first as the primary
  location, and per kind the callee and clause ordinal, or the clause ordinal and contract
  code hash.
- **Signatures,** sorted: every signature the exported definitions and signatures reach,
  with the dependency list its fingerprint covers and, for a `pub opaque` type, its
  owner-private representation, which only definitions of the owning package see.
- **Constants:** the materialized value of every exported constant.
- **`skipped: [{name, reason}]`:** every definition in the closure whose TIR could not be
  exported. Mandatory, possibly empty; a consumer turns each entry into a located error,
  never into silence.
- **Annotations,** sorted by `DefPath`, then source order: every analysis annotation of an
  exported definition as `(Annotation path anchor name payload location)`
  ([PS-17](#ps-17-analysis-annotations)), then `(AnnotationDigest #…)`:
  `H("hirsel.analysis.annotations.v1", section text)`.
- **A trailing `(ExportDigest #…)` node:** `H("hirsel.tir.export.digest.v1", export node text)`.

Selection is every definition of the root package plus its semantic closure (the dependency
walk the semantic program revision uses), so a definition's callees, constants and types are
always present by signature, and by TIR when they have a body.
`hirsel export --tir <source|manifest.json> -o <file>` writes the export.

### PS-10: Import

Import runs out of process, linking only the export, codec, TIR, type and validation
crates. It refuses, with a located error (byte offset as line and column, or the `DefPath`
and field, or the validator violation's body, block and instruction), on:

- any decode error, unknown tag, noncanonical value or trailing input;
- a format, `TIR_VERSION` or interface version other than its own (exact equality);
- an export digest, TIR digest, reference list, obligation table or checksum that does not
  recompute to the stored value;
- a signature fingerprint that does not re-derive: every fingerprint (of a declaration, a
  fact helper or a registry row) has one derivation, its encoding over the dependency list
  it covers, and the importer recomputes it from the exported signature and that list; an
  owner view is consistent exactly when it seals to its public signature. Then every
  dependency, reference and definition signature hash must name those fingerprints;
- any IR validation violation of any definition, against the signature view of its package;
- an annotations section that is not sorted by definition, names a definition without an
  exported signature, anchors a parameter outside a function's parameters or a field or
  payload on a definition that is not a type, spells a name that is not dotted, or whose
  digest does not recompute.

Import returns the definitions, signatures, tables, skip list and annotations. There is no in-process
plugin API: consumers read the file.

## Rules for consumers

### PS-11: Proof never decides

Proof output never decides acceptance or meaning. It can inform, and it can license a
[FACT-3](contracts.md#fact-3-checking) check removal; it never rejects or admits a program
and is never a flag that changes semantics.

### PS-12: Sidecar evidence only

Evidence lives only in a sidecar keyed by `(program revision, ObligationId, checksum)`: the
checksum decides currency ([PS-7](#ps-7-what-digests-bind)); the revision records the
program it was checked for and binds any removal set to one launch. Source never carries
proof syntax or prover hints; source contracts stay executable code
([FACT-1](contracts.md#fact-1-contracts)). Unknown kinds in a newer export are ignored by
older consumers, not misread.

### PS-13: Fail-safe removal

A check-removal pass treats every missing, rejected, unreplayable or stale record as "keep
the check". The low-level IR validator confirms that the set of removed checks equals the
accepted set; any difference is a compile-time tool error, never a silently kept or removed
check.

#### Sidecar format and removal forms (v1)

PS-12 and PS-13 apply to `hirsel build` and
`hirsel run --native` with `--proofs <sidecar>` and `--trust <tool@version>`.

- **Format.** A JSON object `{"format": "hirsel.proof.sidecar.v1", "records": [...]}`.
  Each record has exactly `revision` (the export header's program revision),
  `obligation` (the written PS-1 id), `checksum` (PS-6), `checker`
  (`tool@version`), `budget` (`{"unit", "count"}`) and `verdict` (`proved`,
  `refuted`, `unknown` or `timeout`). Hashes are spelled as in the export: `#`
  and 64 lowercase hex digits. A missing or unknown field, or two records with
  one `(revision, obligation, checksum)` key, refuses the sidecar.
- **Currency.** A record is current for an obligation when its revision is the
  program revision and its checksum the obligation's. A program without a launch
  entry has no revision, so nothing in a sidecar is current for it.
- **Trust.** The checker hook is an explicitly trusted external verdict.
  Nothing is replayed: a current `proved` record is accepted when its checker is
  one the build names with `--trust` and its budget is a positive count in
  `steps`, `heartbeats`, `conflicts`, `rlimit` or `fuel`
  ([PS-15](#ps-15-deterministic-budgets-replayable-evidence)). A record from
  any other checker is unreplayable; a verdict other than `proved` is rejected.
- **Removal forms.** An accepted `check` or `ensures` obligation removes the
  `Check` at each of its sites. The clause is still evaluated, so its own
  failures remain. An accepted SEM-1 `failure-edge` on integer `Add`, `Sub`,
  `Mul`, `Div`, `Rem`, `Shl`, `Shr` or `Neg` removes that operation's checks:
  the unchecked operation is exact whenever its obligation holds. One Div
  obligation covers both zero and signed overflow; signed `MIN % -1` still
  yields zero. An accepted MEM-14 or MEM-13 `failure-edge` removes respectively
  the bounds or active-tag guards of exactly its projected place at its sites;
  other projection guards and storage/ownership checks remain. Every other
  obligation keeps its check and is reported: `requires` (the entry check
  stands for every caller), `requires-at-call`, and `Fail`. Removal applies to
  native code only; the interpreter checks everything.
- **Validation.** Removal happens in SSA import. The low-level IR validator
  compares each discharged unit with a reference import that keeps every check,
  counting failure sites by TIR site, opcode, rule and projected place. The two
  may differ in exactly the accepted guards. Naming events that do not access a
  place have no guard to remove. The stage validator also requires accepted
  evidence for every unchecked projection at its exact source site and place.
  Append fusion carries the original sites through retained instructions and
  derived appending bodies; inserted instructions have no source obligation and
  cannot consume evidence. Every accepted guard in native code must be removed.
  The build reports each record's outcome and counts the obligations that have
  no evidence.

### PS-14: Authority is opaque

A function whose [CAP-2](capabilities.md#cap-2-charged-at-use) authority summary is
non-empty (any known term, or unknown), and any function value of such a type, gets no
defining equation in any encoding: it is world-threaded or uninterpreted per call, so
`clock.now() == clock.now()` is unprovable. This is F*'s `NDET` masking, adopted because
giving such functions equations lets a prover equate two calls that extraction runs
separately, which is enough to prove `False`.

### PS-15: Deterministic budgets, replayable evidence

Budgets are deterministic resource counts (steps, heartbeats, conflicts), never wall time;
wall time is only an outer kill switch and never a verdict. Evidence is proved only when
it replays with its recorded checker version and budget, or when
[PS-13](#ps-13-fail-safe-removal) accepts it from a checker the build names with `--trust`.

### PS-16: Named trust

Floats (bit-pattern semantics against a prover's reals or IEEE model), the projection from
TIR to the consumer's logic, and any native evaluation (`native_decide`, `bv_decide` axioms,
solver-internal evaluation) are named trust items in whatever ledger a consumer keeps; none
is implicit.

## Pluggable analyses

An analysis beyond the type system is optional in Bracha's sense: it has no effect on what
a program means and no annotation is mandatory. It runs out of process over the export,
like a prover, and reports what it finds; the core type system stays the only one the
checker enforces.

### PS-17: Analysis annotations

`@analysis(name)` or `@analysis(name: payload)` states a fact for the analysis `name`
(a dotted name such as `taint` or `acme.units`); the payload is any tokens with balanced
`()`, `[]` and `{}`, and its meaning belongs to that analysis.

```hirsel
@analysis(acme.units: result = m/s)
fn speed(@analysis(unit: m) d: u64, @analysis(unit: s) t: u64) -> u64 {
  d / t
}

type Reading {
  @analysis(unit: m) depth: u64,
  @analysis(taint: secret) site: String,
}
```

An annotation stands before an item (among its attributes), before a parameter of a function
definition, before a record field or a distinct type's field, or before a variant payload.
Anywhere else, including parameters of function types, closures and local functions, it is
LANG-7. An item or site may carry several; source order is kept.

The compiler parses, formats and exports annotations and reads them for nothing else. The
export lists each one with its definition and anchor: `(Item)`, `(Parameter i)` (the TIR
entry body's `parameters[i]`, the signature's parameter `i`), `(Field i)` (a record's field
`i`, a distinct type's field 0) or `(Payload variant field)`, its name, its payload as the
token spellings without trivia, and its location. Annotations of a dependency read from a
precheck are not exported.

### PS-18: Findings sidecar

An analysis reports in a JSON sidecar
`{"format": "hirsel.analysis.findings.v1", "export", "analyzer", "findings": [...]}`.
`export` is the export digest the analysis read (`#` and 64 lowercase hex digits), so a
finding is current exactly for that export: an analysis may follow data through any
definition, so no narrower key is sound. `analyzer` is `tool@version`. Each finding has
exactly `analysis` (the annotation name it reports on), `code` (the analysis's own),
`severity` (`error`, `warning` or `note`), `definition` (a written `DefPath` in the export),
`location` (`{module, line, column, end_line, end_column}` as in PS-8), `message` and
`related` (a list of `{location, message}`). A missing or unknown field refuses the sidecar.
Canonical text sorts keys and indents by two spaces.

A finding is not an obligation: it has no PS-1 id or checksum, and neither PS-12 evidence
nor PS-13 removal reads it.

### PS-19: Analyses never decide

Analysis annotations and findings never change what compiles or what a program means.
Replacing any well-formed annotation with whitespace leaves every diagnostic, the semantic
program revision, every definition hash, TIR digest, obligation id and checksum, and the
program's interpreted and native runs the same; the export differs only in its annotations
section. An item begins after its leading annotations, so a diagnostic over the whole item
is placed as without them, and a recursive type group's key in its members' signatures is
over the members' tokens, never their text. Semantic source revision fields use parsed
item structure without annotations or trivia, so equivalent END spelling after formatting
also preserves replay binding. (Native objects identify each item's source
text in their failure-site codes, so any byte of an item, a comment as much as an
annotation, moves their bytes but not what they do.) No compiler path reads a findings
sidecar, so a finding never rejects a program, admits one or licenses a check removal. The
conformance oracle O5 checks this over the corpus by inserting an annotation at every site
of each program and comparing it with its blanked twin.

## Consumers

Consumers may project exported obligations to Lean, SMT or Why3, or drive an agent proof
loop. An analysis reads annotations from the same export (PS-14 hides which calls hold
authority from no one, only their equations); `crates/hirsel-analysis-canary` keeps a taint
and a units analysis as canaries of the seam. Callee reasoning uses signature contracts; authority follows [PS-14](#ps-14-authority-is-opaque)
and evidence currency follows [PS-7](#ps-7-what-digests-bind). These consumer sketches are
not part of the contract.

What lowering erases, and how a consumer recovers it:

- **Loop structure.** `for` and `while` become control-flow cycles and self-tail sites;
  consumers recover natural loops from back edges, and the header block's location names the
  source loop. An `invariant` clause form will arrive with its own obligation kind and
  anchor.
- **`before(e)` and contract parameter roles.** `before(e)` lowers to an entry temporary
  passed as a clause argument; whether a clause argument is a parameter, a snapshot or
  `result` is recoverable by dataflow from the clause call.
- **Closure contracts** export as `check` obligations with rule FACT-1, not as `requires`
  or `ensures` of the closure.
- **Expression trees** are flattened into temporaries in evaluation order, and
  short-circuit operators become branches; def-use rebuilds them.
- **Operation semantics.** Registry operations have signatures and effect flags but no
  bodies or contracts; consumers treat them as uninterpreted (PS-14 where they hold
  authority).
- **Termination.** There is no `decreases`; consumers prove partial correctness or supply
  their own measure.
- **Failure-site precision.** MEM-13 and MEM-14 failures report at the instruction that first
  touches the place, so a reported site may map to several candidate obligations.
