# Typed JSON hook kit

From the repository root, list every local nominal in the graph and name its
root. The checked library fixture has `Snapshot { captured: time.Date,
items: List[Item] }` and `Item { name: String, active: bool }`:

```sh
tools/json-hooks Item Snapshot --root Snapshot > examples/typed-json-kit/hooks.hir && hirsel check examples/typed-json-kit/check.json && hirsel test examples/typed-json-kit/hirsel.json
```

The manifest is a library package with two source files in the same module:
the hand-authored `main.hir` and the uncommitted `hooks.hir`. `check.json` adds
a no-op entry from `check.hir`, because `hirsel check` requires a launch manifest.
To check the native
engines too, run `hirsel test examples/typed-json-kit/hirsel.json --native --backend
llvm` and repeat with `--backend cranelift`. The four tests check independent
expected JSON, escaped strings, Date, nested unknown values, malformed nested
values, and retained decoded strings/lists after a local input has been dropped.
This exercises the existing semantics; it adds no ownership contract.

`tools/json-hooks` only emits declarations on stdout. It reads no source, rewrites
no file, discovers no nested types, and enables no implicit derive. List nested
local nominals explicitly, including those inside lists or options. Omitting
`Item` produces unresolved `item_from_json_reader` / `item_json_into` calls when
Hirsel checks `Snapshot`. The tool cannot detect that omission or verify that an
identifier actually denotes a nominal: the compiler checks the emitted source.
Do not list `Date`, `Option`, or `List`; the current deriver handles those already.

Arguments must be ordinary ASCII local non-generic identifiers. Qualified/imported
paths (`models.Item`, `models@1/Item`) and generic spellings (`Page[Item]`) fail
with an explanation. The root must be in the supplied list. Duplicate identifiers,
colliding snake stems (`HTTPServer` / `HttpServer`), and collisions between supplied
type names and emitted declarations fail before any source is written. Diagnostics
use stderr and a nonzero exit status. Output preserves argument order and ends in
one newline. Shell redirection itself can truncate a file before any program
runs; write to a fresh scratch file and move it after success if replacing your
own existing file. The fixture's generated path is ignored by Git.

Names exactly follow `std@1/derive.snake`: `HTTPServer` becomes `http_server`,
`ABc` becomes `a_bc`, and `A1B` becomes `a1_b`. Readers and writers are emitted
for every listed type, followed by one root bytes decoder. Fully qualified
`std@1` and `core@1` references avoid depending on convenience aliases; the
manifest still needs its ordinary `std@1` dependency. Hooks are private in the
local module, as in [snapshift](../examples/snapshift/main.hir).

## Exact declarations

For `tools/json-hooks Item Snapshot --root Snapshot`, stdout is:

```hirsel
fn item_from_json_reader(mut cursor: std@1/json.Reader) -> core@1/Result[Item, core@1/String] uses {} = comptime std@1/derive.from_json_reader(
  shape(Item)
)

fn item_json_into(value: Item, mut out: core@1/List[core@1/u8], depth: core@1/u64) -> core@1/Result[
  (),
  std@1/json.JsonError
] uses {} = comptime std@1/derive.json_into(shape(Item))

fn snapshot_from_json_reader(mut cursor: std@1/json.Reader) -> core@1/Result[Snapshot, core@1/String] uses {} = comptime std@1/derive.from_json_reader(
  shape(Snapshot)
)

fn snapshot_json_into(value: Snapshot, mut out: core@1/List[core@1/u8], depth: core@1/u64) -> core@1/Result[
  (),
  std@1/json.JsonError
] uses {} = comptime std@1/derive.json_into(shape(Snapshot))

fn snapshot_from_json_bytes(data: core@1/Bytes) -> core@1/Result[Snapshot, core@1/String] uses {} = comptime std@1/derive.from_json_bytes(
  shape(Snapshot)
)
```

## Imported nominals

The current deriver resolves an imported field's hooks in the type's defining
module. For a field `models@1/Item`, add these declarations manually **in that
package's defining module**, and export them for callers:

```hirsel
pub fn item_from_json_reader(mut cursor: std@1/json.Reader) -> core@1/Result[Item, core@1/String] uses {} = comptime std@1/derive.from_json_reader(
  shape(Item)
)

pub fn item_json_into(value: Item, mut out: core@1/List[core@1/u8], depth: core@1/u64) -> core@1/Result[
  (),
  std@1/json.JsonError
] uses {} = comptime std@1/derive.json_into(shape(Item))
```

The importing package lists only its own nominals. A local wrapper with a new
name does not replace the defining module's required hook. If you cannot change
that dependency, provide its supported explicit conversion at your application
boundary instead. Generic nominals require manually authored wiring appropriate
to their type parameters; this helper does not infer it.

## Verification and prior art

```sh
python3 tools/buck2/prepare.py
.buck2/bin/buck2 test root//:test-json-hooks root//:test-json-hooks-integration
```

The integration target generates into a temporary package, first proves the
missing-nested-hook negative control, checks the complete graph, then verifies
the exact four-case inventory and nonzero success count on H, LLVM and Cranelift.
The fixture stays hand-authored; no generated Hirsel output is committed.

The per-nominal expansion structure follows Serde's
[`serde_derive/src/de.rs:25–75`](https://github.com/serde-rs/serde/blob/0db9b3b504bcb0a7fa9cf001429869519afd3e9a/serde_derive/src/de.rs#L25).
The unknown-before-known and nested-sequence vector follows
[`test_suite/tests/test_annotations.rs:425–462`](https://github.com/serde-rs/serde/blob/0db9b3b504bcb0a7fa9cf001429869519afd3e9a/test_suite/tests/test_annotations.rs#L425).
Serde is MIT OR Apache-2.0; these are structural/test-vector adaptations, with no
Serde implementation copied. Hirsel-specific deviations: no Rust trait, macro,
source parser, generic inference, or lifetime machinery; the tool emits only
existing Hirsel `comptime` hooks. The vector uses JSON bytes and extends the
unknown sequence with objects/null and a nested record boundary. Duplicate keys,
malformed skipped values, required fields and invalid Dates keep Hirsel's existing
error policy. Naming and signatures come from `std@1/derive.hir:151–182,
1740–1848` and `examples/snapshift/main.hir:1–32`, verified against planning pin
`ed52d351c7fde61d8fd08267c0100d61aff71750` and the launch source.
