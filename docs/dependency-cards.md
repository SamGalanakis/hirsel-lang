# Dependency cards for agents

Run `hirsel deps card <manifest.json>` before choosing, updating or replacing a
package. It reports computed facts for every selected dependency, including
transitive dependencies. This command groups dependency inspection under `deps`;
`card` leaves room for other dependency operations.

```sh
hirsel deps card examples/jsonquery/hirsel.json --package std@1/json
hirsel deps card packages/dep_card_fixture/app/hirsel.json --json
hirsel deps card examples/jsonquery/hirsel.json --package std@1 --json
```

`--package name@version` selects an exact identity already in the manifest graph.
Append `/module/path` to inspect one module subtree. Aliases do not change the
identity. An explicit selector may also select the root package. Without a
selector the root is excluded. A manifest without an entry uses its public API
as roots and labels `reach_basis` as `public_api`; a launch manifest uses its
entry and labels it `entry`.

The versioned [JSON schema](dependency-card.schema.json) describes `--json`.
Arrays are sorted by definition identity and contain no machine-specific source
paths or wall-clock timestamps. Unknown evidence is explicit, never an empty
list pretending to prove safety.

- **Authority:** `whole_public_api` closes over every exported non-test function;
  `reached_by_you` closes over this dependency's definitions reached from the
  importing program's roots. Both reuse the engine's semantic dependency graph,
  including private helpers, finalizers, compile-time reads and implicit calls.
  `capabilities` unions callable `uses` summaries, specializing parameter terms
  with the compiler's structural authority algorithm. `operations` names the
  canonical core operations with capability-bearing operands, such as
  `core@1/net/connect`. An unrestricted or unresolved summary is listed under
  `unresolved`; generic callbacks can contribute authority unknown before an
  instantiation. These are conservative static closures, not dynamic call counts.
- **Weight:** definitions are top-level source declarations; units are manifest
  source files, including empty files in the total. Reached counts use the same
  semantic closure as a build, so type and metadata dependencies also count.
  Transitive dependencies are the manifest's pinned package identities. Module
  cards retain the enclosing package's dependency list.
- **Cost:** `source_token_work_estimate` counts significant source tokens in
  reached declarations. `check_source_tokens` and `lower_source_tokens` estimate
  body work remaining after matching prechecks; `native_build_source_tokens`
  estimates the reached source weight still presented to native compilation.
  A precheck saves checking/lowering, not native code generation. These are work
  proxies, not milliseconds or instruction predictions. The cache stores no
  per-unit check/build timings, so `check_ms` and `build_ms` are null. No native
  build, benchmark or test run is started to fill them in.
- **Exposure:** dependency type identities in the root's exported signatures,
  including nested callback and generic argument types, with the signatures
  exposing each. Written aliases are retained alongside normalized types.
  Private body types and opaque representations do not count.
- **Determinism:** externs, core Unsafe operations and unmodeled foreign callers
  in both closures. `unreplayable` follows the engine's deterministic-wrapper
  rule. `no_unmodeled_ffi` says only that this particular obstruction is absent;
  capabilities can still require recorded inputs or host support. Hirsel uses
  explicit Unsafe capability operations rather than syntactic unsafe blocks.
  `provider_requirements` names capability contracts; concrete provider versions
  come from the run record, which a source manifest does not select.
- **Health:** count of source `@test` declarations and package-owned `.hir` corpus
  files recursively under `corpus/` beside its manifest. Module cards report that
  enclosing package's corpus count. Consumer suites elsewhere are not counted
  as package-owned cases. Counts do not assert that tests passed. `last_gate` is
  null: the build cache has no content-bound per-package test receipt.

The normal loader, shared query cache and matching toolchain prechecks are reused.
Without a matching precheck, the normal compiler checks from source. Whole-API
inspection can select bodies that the importing program alone never needed;
existing checked/lowered products are reused. Changing comments or source order
does not invent provenance, test passes, timings or stronger authority claims.

For code-store evidence, pass the path to a code-store configuration entry:

```sh
hirsel deps card packages/dep_card_fixture/app/hirsel.json \
  --code-store /srv/hirsel/code-store.json --json
```

The configuration uses the [code-store providers](../crates/hirsel-code-store/README.md),
for example `{"provider":"code.dir@1","path":"/srv/hirsel/code"}`.
The card computes the LANG-6 hashes of the selected public API closure and looks
up its package-owned objects. Present objects are rehashed on read. Under
`sign.ed25519@1`, the store also verifies a trusted signature; the card reports
the signing policy, trusted keys and per-object verification. Missing objects
are marked `present: false`; corrupt or unsigned objects under a signing policy
fail the command. With `sign.none@1`, `signature_verified` stays null. Without an
explicit store, `provenance` is null. This inspection does not publish code.
