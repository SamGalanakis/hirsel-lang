# Type analysis extraction

The split adds `hirsel-type-analysis`, whose public modules own cleanup summaries
and specialization, parametric uniqueness, logical arena sizing, diagnostic type
spelling, and nominal representation expansion. Consumers import each module
directly; there are no re-export shims or compatibility aliases.

Stable definitions, canonical CT spelling, graph transport, basic property and
crossing queries, and the nominal memoization they use stay in `hirsel-types`.
Cleanup-bound graph normalization stays with the shared property normalizer.
Moving that shared cluster would require a larger graph/property extraction.
The independent-analysis boundary takes priority over maximizing the churn share.
No language rules, IR definitions, runtime ABI or encoding algorithms change.

## Dependency counts

Counts are workspace crates with a path through normal/build dependencies;
parentheses include dev dependencies. The crate itself is excluded. The final
counts compare the rebased parent `fd6503bf` with this extraction, so concurrent
crate splits are not attributed to this change.

| Crate | Direct before | Transitive before | Direct after | Transitive after |
| --- | ---: | ---: | ---: | ---: |
| hirsel-types | 17 (21) | 21 (27) | 18 (22) | 22 (28) |
| hirsel-type-analysis | 0 (0) | 0 (0) | 8 (10) | 14 (28) |

At the original churn baseline `9dbe208974b3`, hirsel-types had 14 direct and
18 transitive production dependents (18 direct and 24 transitive including tests).

The eight production consumers are check, signature, authority, ownership,
validate, engine, interpreter and driver. LIR uses logical sizing only in tests.
New differential parametric tests added concurrently share the original crate's
property-test fixtures in a new integration target, so hirsel-types imports
analysis only as a dev dependency for those tests. Integration tests keep both
sides on the same library TypeStore rather than the unit harness copy. This accounts for the broad test-only reach; no
production cycle exists. TIR, layout, modes and native backends do not acquire
a production dependency.

## Three-day churn

The fixed window ends at baseline `9dbe208974b3` (2026-10-20 12:31:23 +02:00)
and begins 2026-10-17 12:31:23 +02:00. There are 46 commits touching
`crates/hirsel-types` in this window. Churn counts additions plus deletions,
including tests, documentation and manifests. The extraction itself is excluded.

Before the split, all 3,216 changed lines belonged to `hirsel-types`. Projecting
those same edits through the new ownership boundary gives:

| Ownership after extraction | Three-day changed lines | Share |
| --- | ---: | ---: |
| hirsel-types | 1,842 | 57.3% |
| hirsel-type-analysis | 1,374 | 42.7% |

| Moved source region | Changed lines transferred |
| --- | ---: |
| logical.rs | 409 |
| parametric.rs | 448 |
| Diagnostic spelling, excluding canonical CT spelling | 478 |
| Cleanup summaries/specialization and their seal test | 32 |
| Representation expansion and its four tests | 7 |

The largest retained concentration is nominal proof memoization (537 changed
lines); it serves the basic graph/property queries. Canonical CT spelling retains
135 lines of spelling churn.

To reproduce the baseline, run `git log 9dbe208974b3 --since=2026-10-17T12:31:23+02:00
--numstat --format= -- crates/hirsel-types` and sum the numeric columns. For each
commit, project `git diff --no-renames -U0 <commit>^ <commit> --
crates/hirsel-types` by the changed line's ownership in the corresponding old/new
version: logical and parametric move entirely; spelling before the canonical CT
spelling declaration moves; properties' cleanup prefix and seal cleanup tests
move; nominal's representation-failure/expansion region and four representation
tests move. Everything else remains in the original crate.

Rustc time was not measured; there is no timing or speedup claim.

## Verification

At the original baseline, 102 test definitions become 94 plus eight. After
concurrent differential tests landed, the rebased parent has 106 definitions:
98 remain in hirsel-types and eight move to hirsel-type-analysis. No test
behavior is removed or newly invented by the extraction.
Four representation tests, two logical-size tests, one diagnostic-spelling test
and one cleanup test move with their code.

Build files are regenerated with `tools/buck2/sync.py` and are not committed.
Workspace build, Clippy, formatting, all affected crate targets and selected
dependents were checked by the earlier targeted run. It passed 62 Buck2 test
targets and all five selected corpus cases. Its layering-allowlist failure is
fixed here; three targets that the pool never ran are retried directly. After
rebasing, the affected authority and ownership tests, new analysis tests,
original type tests, layering tests, interpreter and validator tests run directly
with Buck2. `tools/land` builds the workspace and checks Clippy and formatting
before pushing.

Direct reruns passed: type unit tests (82), shared differential tests (6), new
analysis tests (8), interpreter integration tests (96), validator integration
tests (473), authority tests (26) and ownership tests (28). The LIR integration
target passed 323 tests with one ignored; its layering assertion exposes a
pre-existing `hirsel-proof-export -> serde_json` production dependency forbidden
by its allowlist, now recorded in tools/known-failures.tsv. Both the new analysis
crate and the concurrently added canary have their correct layering classes.

The selected existing corpus cases cover parametric tuple callback results,
nominal cleanup under Map/SortedMap, and packed bounded storage, including their
twins. No whole test campaign is selected. The moved implementation bodies match
the original baseline after imports, formatting and unused-binding cleanup.
