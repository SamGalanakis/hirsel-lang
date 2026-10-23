# policycheck

Offline role and resource policy evaluation with ordered explanations and atomic
in-memory policy replacement. Run with the included read-only grant:

```
hirsel run examples/policycheck/hirsel.json --grants examples/policycheck/grants.json
```

The actual entry takes `Io` and `Args`. Arguments are `ROOT FILE JOBS`, optionally
followed by `--interruptible`; `--help` prints usage. Jobs must be1..4. The input
and full output are capped at128KiB and64KiB. stdout contains one JSON result;
stderr contains `{"error":"code"}` on fatal input, grant or cancellation errors.
Exit0 means completed, including denied queries and rejected reloads; exit2 is
a fatal workflow error; exit3 means stdout/stderr was unavailable.

A v1 document has exactly `version` and `operations`. Each operation is either
`{"reload":POLICY}` or `{"batch":[QUERY...]}`. At most32 operations and32 queries
per batch are admitted. A policy has exactly `roles`, `edges`, `principals` and
`rules`. See [fixtures/workflow.json](fixtures/workflow.json) for a full example.

- Roles are distinct strings. Edges have `role` and `parent` fields. A role
  inherits its parents. Principal records contain `id` and a list of `roles`.
- Rules contain `id`, `effect` (`allow`/`deny`), `role`, `action`, `path`, `owner`.
  Action and owner use exact equality, or the whole-field wildcard `*`.
- Queries contain `id`, `principal`, `action`, `resource`; a resource contains
  `path` and `owner`. Unknown principals and absent policies deny by default.
- IDs and action/owner values are1..32 UTF-8 bytes. Paths are absolute,1..256
  bytes, using ASCII alphanumeric/underscore/hyphen/dot segments. Empty segments,
  `.` and `..` are rejected. A rule may end in one `/*`: `/data/*` matches
  `/data/a` and `/data/a/b`, excluding `/data` and `/database/a`. `/*` excludes
  the root itself. Literal `/` is admitted. No regex or dynamic code is evaluated.

Policies admit64 roles,64 principals,128 edges,128 rules,64 grants/principal
and16 edges on every inheritance path. Cycles, unknown roles, duplicate role,
principal, rule, edge or grant IDs reject a reload. Duplicate query IDs reject
a batch. Unknown/missing fields and wrong types fail shape validation.

A reload validates its complete replacement before changing the installed
shared policy. A rejected reload preserves the revision and prior policy. Each
batch copies that shared handle once, then schedules independent query tasks in
windows of at most JOBS. Results own their explanations and publish in query
order. All matching rule IDs appear in declaration order. Authorization requires
at least one matching allow and no matching deny. The example changes no Hirsel
capabilities and offers no network service.

Results contain reload outcomes or batch revisions and decisions, as shown by
the frozen independent vectors in
`crates/hirsel-cli/tests/support/policy_evaluator_33/cases.json`. A complete result
is buffered and checked before stdout publication. Output overflow publishes
only a fatal diagnostic. This is atomic in-memory policy replacement, not a
persistent policy database or a stdout transaction across provider failures.

[LICENSES.md](LICENSES.md) records pinned Casbin sources and deviations.
[FRICTION.md](FRICTION.md) records qualification limits without treating stopped
or unrun cells as passed.

Qualification keeps original stop/failure identities: the50 frozen vectors run
through the real Io+Args entry. Six cap vectors reach the fixed1M interpreter
step budget; native oracle execution and each replay have separate receipts.
Physical Ordered widths1/2/4 are deferred to the exact-CPU deployment owner.
These are not all-engine or physical-parallelism claims. See FRICTION.md.

Focused verification (run in the owned checkout):

```
python3 tools/buck2/prepare.py
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=policy_evaluator_33_frozen_oracle --test-arg=--nocapture
.buck2/bin/buck2 test -c hirsel.tests=host //:hirsel-cli-test-cli-shard-1 -- --env POLICY_EVALUATOR_33_EVIDENCE_DIR=/absolute/external/evidence --env POLICY_EVALUATOR_33_CASE=inherited-allow --test-arg=policy_evaluator_33_native_record_replay --test-arg=--nocapture
```

`POLICY_EVALUATOR_33_CASE` is a comma-separated selection of frozen case IDs.
Use selections of at most10 native cases under the ordinary outer test limit.
The campaign preflights a private std image copy, seals the actual inputs, then
runs one child at a time under60CPU-s/2GiB. Never raise those caps to accept an
original vector. `POLICY_EVALUATOR_33_ENGINE=reference|checked-h` selects missing
interpreter cells; `POLICY_EVALUATOR_33_RESUME_ROOT` reuses explicit accepted
checkpoints for a single case. Retain original attempt directories unchanged.
