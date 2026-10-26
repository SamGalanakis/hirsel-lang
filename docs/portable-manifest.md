# Check in a portable package manifest

`hirsel new` selects the installed toolchain's resolved absolute std path.
For a package checked into a workspace containing its matching Hirsel checkout,
run the checked helper immediately after `new`:

```sh
# Layout: workspace/hirsel/ is the matching checkout; workspace/demo/ is new.
cd workspace
hirsel new demo
hirsel/tools/portable-manifest demo/hirsel.json \
  --workspace-root . --std-manifest hirsel/stdlib/std@1/hirsel.json --write
hirsel check demo/hirsel.json
hirsel test demo/hirsel.json
```

The std dependency becomes `../hirsel/stdlib/std@1/hirsel.json`. In this
repository, for a package created with `hirsel new examples/demo`, use:

```sh
tools/portable-manifest examples/demo/hirsel.json \
  --workspace-root . --std-manifest stdlib/std@1/hirsel.json --write
```

This produces `../../stdlib/std@1/hirsel.json`, as in
[snapshift](../examples/snapshift/hirsel.json). Select the checkout matching your
toolchain explicitly: verifying the identity `std@1` does not prove compatible
compiler and library revisions. `hirsel new` keeps its existing defaults.

Omit `--write` to review deterministic, sorted, two-space JSON on stdout. Dry
output leaves the input bytes untouched; errors emit no partial JSON to stdout.
`--write` emits no stdout and replaces only the input manifest, after the whole
request validates, using a temporary file in the same directory and an atomic
rename. It preserves the input's permission bits and refuses a symlink input.
Ownership, timestamps, ACLs and extended attributes are not preserved.

The helper requires Python 3.9 or later and accepts one schema-1 or schema-2
package manifest, a required `--workspace-root ROOT`, and an optional
`--std-manifest PATH`. Command-line paths are relative to the process's working
directory. Dependency paths are relative to the input manifest's canonical
parent directory, following the compiler's manifest loader, regardless of cwd.
Every existing dependency target must have schema 1 or 2 and declare the exact
package name and canonical decimal u64 major version requested by its edge.
Package names follow the compiler's ASCII identifier and reserved-word rules.

Schema 1 rewrites `dependencies[*].manifest`; schema 2 rewrites
`dependency_catalog[ID].manifest`. Catalog IDs, package identities, parameters,
signatures, defaults, whole binding maps, forwarding and exposures retain their
values. Catalogs are lazy: a missing relative target is preserved verbatim when
its resolved path stays inside the workspace, including through existing
symlink prefixes. Its identity cannot be checked. This also permits a default
whose target is absent when a caller overrides it. The helper does not decide
which entries the compiler selects; `hirsel check` rejects a missing target if
selection actually needs it. Missing absolute targets are refused: keeping their
spelling would leave the workspace tied to the old location. All catalog paths,
even unused ones, receive local-path and containment checks. Existing targets
receive identity checks.

An explicit std replacement requires exactly one dependency descriptor whose
**package identity** is `std@1`; a filename containing `std` does not select it. The target
must identify itself as `std@1`. Its original path may be unavailable or outside
the workspace because that selection is replaced before path resolution. No
other dependency is replaced or inferred. Duplicate `std@1` descriptors
(including catalog aliases) are rejected with or without a replacement. Std-free packages remain std-free.

All final dependency selections and the input manifest must resolve inside the
canonical workspace root, including through symlinks. Output paths use the
resolved layout. Contained symlinks may be followed; symlinks escaping the root
are refused. URLs, scheme references (including `file:`), UNC paths, foreign
drive references, missing required files, mismatched identities, duplicate JSON
keys, non-JSON numbers and unsupported schemas/shapes fail before output or writes.
Validation is **not a filesystem security boundary against concurrent writers**;
use a stable workspace while running the helper.

Unknown JSON fields and all nondependency values are preserved semantically,
including the spellings of sources, entry and grants. Only each direct dependency
descriptor's `manifest` value changes. JSON whitespace and key order are normalized. The helper validates selection identities and local manifest paths;
ordinary `hirsel check` / `hirsel test` still validate sources and language rules.

Portability means moving the **same relative workspace layout**. This command
does not copy sources, vendor or download dependencies, lock revisions, rewrite
transitive manifests, rewrite source/grant paths, or change toolchain installation
policy. Transitive manifests and any absolute source/grant paths need their own
review. Keep installed absolute paths when deliberately tying a package to an
installed toolchain.

## Verification and prior art

```sh
python3 tools/tests/portable_manifest.py Unit
# Registered targets, with the matched compiler/runtime on the execution pool:
.buck2/bin/buck2 test root//:test-portable-manifest root//:test-portable-manifest-integration
```

The integration creates a tiny std-free package and a sibling library with a
space/Unicode directory name, rewrites the package, copies the whole workspace,
and deletes the original. It checks the moved manifest, requests its ordinary
dependency card, and runs exactly two tests on the interpreter and Cranelift.
This makes fallback to the original dependency impossible. Std replacement is
covered separately by the tool unit tests. The driver case
`package_slots::portable_catalogs_preserve_effective_slots_and_execution_after_moving`
uses the compiler slot fixture (schema-1 provider, default, override, suppressed
missing default and forwarded binding), asserts the terminal slot maps and
successful execution before rewriting, after rewriting and after moving the
workspace, and checks byte idempotence of every rewritten manifest.

Relative derivation follows Go `path/filepath.Rel` at commit
`ebe2feefc400f813aefdc8a036e66d2619da6649`,
[`src/path/filepath/path.go:185–264`](https://github.com/golang/go/blob/ebe2feefc400f813aefdc8a036e66d2619da6649/src/path/filepath/path.go#L185),
using Python's standard-library `os.path.relpath` for the same shared-prefix,
parent-components, then target-components structure. No Go implementation text
is copied. Equal, sibling, descendant, ancestor and dot-component vectors are
adapted from
[`path_test.go:1495–1538`](https://github.com/golang/go/blob/ebe2feefc400f813aefdc8a036e66d2619da6649/src/path/filepath/path_test.go#L1495).
Unlike Go's lexical interface, rewritten targets here are existing canonical paths
(safe missing relative catalog targets are preserved without derivation): lexical
`..` ambiguity and mixed absolute/relative roots are eliminated before relative
derivation. Absolute and relative spelling variants therefore share one
resolved-path vector; incompatible Windows volumes are checked with `ntpath`
and workspace-root escapes are rejected before derivation. Hirsel's existing
manifest shapes and identity rules supply the package-specific validation.

The adapted Go test vectors are Copyright 2009 The Go Authors. All rights
reserved; their [BSD-3-Clause license and disclaimer](../third-party/NOTICES.md#bsd-3-clause-license-go)
apply. The upstream copyright, conditions and disclaimer are retained there.
