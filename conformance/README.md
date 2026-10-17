# Conformance fixtures

Each `.hir` file is one case, with a `// rule:` and `// expect:` header. The
runner discovers these files recursively and checks their canonical formatting.
A rejected case needs an accepted twin that isolates the rule being tested.

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
