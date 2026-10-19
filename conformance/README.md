# Conformance fixtures

Each `.hir` file is one case, with a `// rule:` and `// expect:` header. The
runner discovers these files recursively and checks their canonical formatting.
A rejected case needs an accepted twin that isolates the rule being tested.

An executed case (`expect: run`, `fail`, `blocked` or `test`) runs under a
budget of 1,000,000 interpreter steps, in ordinary and checked interpretation
alike. A case that needs more raises the budget for itself alone with a
`// steps: <n>` header line, where n is a whole number above 1,000,000 and at
most 20,000,000, immediately followed by a one-line comment saying why the case
needs it, as in [this case](PROG/PROG-3-steps-header-run.hir):

```hir
// rule: PROG-3
// expect: run
// steps: 3000000
// forty thousand loop iterations take over 1.6 million steps
```

Every engine configuration that counts steps honours it: ordinary and checked
interpretation, the native harness's reference interpretation and H, whose
instruction limit (8,388,608 by default) rises by the same factor n / 1,000,000.
Compiled native executables count no steps. The runner rejects a larger or
smaller value, a duplicate, a missing reason and the header on a case that is
not executed.

For multiple packages, put an exact `case.hir.json` sidecar beside the case.
It is an ordinary schema-1 launch manifest: `package` fixes the importing
package identity, `sources` assigns files to explicit modules, `dependencies`
selects other package manifests and exposed heads, and `entry` selects the
function to run. Paths are relative to the manifest declaring them. Dependency
manifests may omit `entry`; put their source files under `packages/fixtures/`
so they are not discovered as separate cases needing expectation headers.

For example, [the external fact constructor](FACT/FACT-2-external-constructor-ok.hir)
uses [its sidecar](FACT/FACT-2-external-constructor-ok.hir.json) to select
[the facts package](../packages/fixtures/facts@1/hirsel.json). Both CLI spellings
load that same graph:

```sh
hirsel check conformance/FACT/FACT-2-external-constructor-ok.hir
hirsel run conformance/FACT/FACT-2-external-constructor-ok.hir.json
```

The conformance runner, checked interpreter and source/IR agreement campaign
retain the sidecar's package identities and dependency sources. No parent
manifest or source directory is searched implicitly. The manifest schema and
lookup rules are defined in [programs and packages](../docs/spec/programs.md).

The `LANG-4-external-*` twins cover ambiguous heads, full versioned paths,
own-package precedence, bare head/value namespaces and duplicate exposures.
The LANG-13 and DROP-4 external twins keep the function text and module segments
identical while changing package identity, alongside the existing cross-module
controls. Existing PROG-2 package fixtures cover opaque construction, reading and
destructuring. The external opaque bound twins exercise plain, flat, copyable,
data, key, noalloc_drop, sendable (MODE-4) and inert (CAP-2) promises; the hidden
scalar representation cannot supply an omitted exported bound. The SER-1 external
twins require a transparent serializable schema and reject an opaque value even
when it exports data, inert and other structural bounds.
