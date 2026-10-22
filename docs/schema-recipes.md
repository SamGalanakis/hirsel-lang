# Explicit nested schema converters

The [checked example](../examples/schema-recipes/main.hir) converts
`Outer { service: Service, retry: Retry }` in both directions. It uses the
current `std@1/derive.schema_decode` and `schema_encode` APIs, with explicit
hooks for each nested nominal type. Run from the repository root with a matching
`hirsel` on PATH:

```sh
hirsel check examples/schema-recipes/hirsel.json
hirsel run examples/schema-recipes/hirsel.json
hirsel run examples/schema-recipes/hirsel.json --native --backend llvm
hirsel run examples/schema-recipes/hirsel.json --native --backend cranelift
```

Every command exits 0 with no stdout or stderr. The root-free entry needs no
grants. Native commands need the pinned LLVM, link kit and runtime configuration
in [building](building.md#what-a-native-build-needs). Each command above runs
once per backend; for repeated observations, follow the
[build-once native recipe](native-verification.md).

The exact [manifest](../examples/schema-recipes/hirsel.json) is:

```json
{
  "dependencies": [
    {
      "expose": [],
      "manifest": "../../stdlib/std@1/hirsel.json",
      "package": { "name": "std", "version": "1" }
    }
  ],
  "entry": { "module": [], "name": "main" },
  "package": { "name": "schema_recipes", "version": "0" },
  "schema": 1,
  "sources": [{ "module": [], "path": "main.hir" }]
}
```

Dependency paths resolve relative to the manifest. No hook-generation tool or
uncommitted generated file is needed.

## Declare the complete graph

```hirsel
type Service {
  name: String,
}

type Retry {
  attempts: u32,
}

type Outer {
  service: Service,
  retry: Retry,
}

fn outer_schema_decode(wire: json.Value, mut check: schema.CheckState) -> Result[
  unique Outer,
  schema.SchemaError
] uses {} = comptime derive.schema_decode(shape(Outer), schema.strict_policy())

fn outer_schema_encode(value: Outer, mut check: schema.CheckState) -> Result[
  unique json.Value,
  schema.SchemaError
] uses {} = comptime derive.schema_encode(shape(Outer), schema.strict_policy())

fn service_schema_decode(wire: json.Value, mut check: schema.CheckState) -> Result[
  unique Service,
  schema.SchemaError
] uses {} = comptime derive.schema_decode(shape(Service), schema.strict_policy())

fn service_schema_encode(value: Service, mut check: schema.CheckState) -> Result[
  unique json.Value,
  schema.SchemaError
] uses {} = comptime derive.schema_encode(shape(Service), schema.strict_policy())

fn retry_schema_decode(wire: json.Value, mut check: schema.CheckState) -> Result[
  unique Retry,
  schema.SchemaError
] uses {} = comptime derive.schema_decode(shape(Retry), schema.strict_policy())

fn retry_schema_encode(value: Retry, mut check: schema.CheckState) -> Result[
  unique json.Value,
  schema.SchemaError
] uses {} = comptime derive.schema_encode(shape(Retry), schema.strict_policy())
```

`shape(Outer)` supplies the *field types*. A `Service` field calls
`service_schema_decode` on read and `service_schema_encode` on write; `Retry`
calls `retry_schema_decode` / `retry_schema_encode`. The field's label does not
choose the hook. Names use the existing derive snake-case rule; a nominal's
hooks resolve in its declaring module. Export hooks needed across module/package
boundaries. Nested records inside containers still need their explicit hooks.
Built-in scalar fields such as `String` and `u32` use the standard converters.
An explicit `Policy.leaves` pair can name custom conversions; it does not
introduce implicit discovery. See [paired structural generation](spec/schema.md#paired-structural-generation).

The generated body constructs the outer record, calling the written nested
hooks. It does not generate those hook declarations. Here is the rejected
[decoder](../crates/hirsel-driver/tests/fixtures/schema-recipe-11/missing-decode.hir)
beside its accepted repair (the complete graph above):

| Rejected | Accepted repair |
| --- | --- |
| Declare the three types and only `outer_schema_decode`. | Also declare `service_schema_decode` and `retry_schema_decode` with the headers above. |
| Declare the three types and only `outer_schema_encode`. | Also declare `service_schema_encode` and `retry_schema_encode` with the headers above. |

Generated nested-hook errors also label the application field's type, for
example `Outer.service`, with the required callable contract:

```hirsel
fn service_schema_decode(value: json.Value, mut check: schema.CheckState) -> Result[unique Service, schema.SchemaError] uses {}
fn service_schema_encode(value: Service, mut check: schema.CheckState) -> Result[unique json.Value, schema.SchemaError] uses {}
```

The label uses fully qualified names, includes variant payload labels (or their
zero-based position), and follows container elements back to their application
field. Text and JSON diagnostics carry the same label and source location.
Wrong hook types retain their ordinary type or generic diagnostic, enriched with
this context. Hooks are still explicit; the contract is guidance, not an
additional signature restriction or a generated repair.

Both missing-hook fixtures reject under [LANG-4](spec/language.md#lang-4-names),
at the unresolved hook name in the generated outer body, with a secondary
span on the original `comptime derive.schema_decode(...)` or
`schema_encode(...)` expression. The tests pin those name spans and their
original source expression, not the full diagnostic wording. Adding only a decoder does not repair an
encoder. The runnable example is the accepted twin for both directions.

## Parse, check, construct, write

Wire JSON is text/bytes and the `json.Value` representation. `json.parse` checks
JSON syntax and rejects duplicate decoded object keys, including `name` together
with `na\u006de`. It does not establish that a value has the `Outer` shape.
`outer_schema_decode` checks that shape and constructs an owned DTO;
`outer_schema_encode` checks the paired policy while constructing an owned
`json.Value`. `json.stringify` writes the compact JSON text.

The example's `decode` and `encode` helpers show this sequence with explicit
error handling. Each complete conversion starts with
`schema.check_state(schema.default_options())`. Pass the *same* `mut check`
through all nested calls in that direction, so depth, work and issue limits
apply to the whole operation. Start a fresh state for the other direction.
The helpers reduce errors to short strings for the executable checks; the
`strict_errors` and `limits` functions inspect the actual typed errors.

Inputs to these hooks are borrowed. `Result[unique Outer, ...]` and
`Result[unique json.Value, ...]` promise fresh owned successful results;
generated bodies are checked against those headers. An ordinary handwritten
constructor must also own counted children:

```hirsel
fn owned(name: String, attempts: u32) -> unique Outer uses {} {
  Outer(service: Service(name: unique name), retry: Retry(attempts: attempts))
}
```

`unique name` obtains independent storage when needed; `take value` moves an
owned result out of a match arm. `copy` shares counted storage and does not
repair a borrowed-result lifetime. Follow the current
[ownership rules](spec/ownership.md) and [modes](spec/ownership.md#passing-modes).
Function-value uniqueness qualifiers must match exactly: storing these hooks in
`schema.Boundary`'s ordinary-result callback fields requires a pure adapter
that matches the hook result and returns `Ok(take value)`. Generated nested
converters supply that adapter internally; this example calls hooks directly.

## Strict wire expectations

Use `schema.strict_policy()` in **all six declarations**. It retains declared
names, requires every field, rejects unknown fields, and supplies no defaults,
aliases or coercions. Null is invalid for the `String` and `u32` fields here.
The writer emits fields in declaration order: `service`, then `retry`;
`name` and `attempts` appear in their respective nested objects. Input order
may differ. Whitespace and input order are not preserved. The independent
encode golden is:

```json
{"service":{"name":"雪"},"retry":{"attempts":3}}
```

The executable checks cover:

| Input or operation | Expected result |
| --- | --- |
| Reversed outer member order, Unicode `雪` | Decode fields, encode exact golden, decode again. |
| `{}` or missing `retry` | `Invalid`, `Missing` at `.service` or `.retry`. |
| Empty `service` / `retry` object | `Missing` at `.service.name` / `.retry.attempts`. |
| Numeric `service.name` / text `retry.attempts` | `Type` at the nested field. |
| Extra member in `service` / at root | `UnknownField` at `.service.extra` / `.extra`. |
| Literal or escaped duplicate `name` | `json.JsonError.DuplicateKey` before schema traversal. |
| `max_depth = 1` | `Limit(Depth, 1)` on both decode and encode. |
| `max_work = 1` | `Limit(Work, 1)` on both decode and encode. |
| DTO/text returned after local inputs end | Owned values remain usable. |

The strict invalidity vectors assert one issue, its code, path and `Wire` stage
under default `First` mode. Limit outcomes are separate from invalidity, and
neither returns a partial DTO. The roundtrip also encodes a separately
constructed DTO against the golden, so mutually incorrect read/write mappings
cannot satisfy it merely by agreeing with each other.

Application validation and migration are a further boundary. A type-correct
name might still be unacceptable to an application; moving old fields into
new nested records needs explicit application code and constructors.
`schema_evolution.compare` reports directional compatibility findings. It does
**not execute migration**, rewrite input, run an adapter or populate new fields.
See [schema evolution](spec/schema.md#explicit-evolution) and
[configctl's explicit migration](../examples/configctl/model.hir).

## Run the correctness tests

With the checkout prepared for Buck2:

```sh
.buck2/bin/buck2 test //:hirsel-driver-test-it-shard-1 -- --test-arg=schema_recipe_11
```

Require five selected tests: two missing-hook rejections and three accepted
executions (roundtrip/ownership, strict wire, and limits in both directions).
The nested-hook diagnostic matrix is checked separately:

```sh
.buck2/bin/buck2 test //:hirsel-driver-test-it-shard-1 -- --test-arg=schema_hook_diagnostics_17
.buck2/bin/buck2 test //:hirsel-cli-test-cli-shard-1 -- --test-arg=schema_hook_diagnostics_17
```

Require ten driver tests and three CLI tests, including missing and ill-typed
hooks, record/variant nesting, cross-package hooks, UTF-8 locations and manual
repairs executed on both native backends.

Zero selected tests is a failure. The four CLI commands above additionally
check the complete entry through the interpreter, LLVM and Cranelift.

## Prior art and adaptations

The explicit Service/Retry hooks start from
[`examples/configctl/model.hir:1–65`](https://github.com/SamGalanakis/hirsel-lang/blob/9f149f01207d15f7bb62c501e3c43823a62f16b5/examples/configctl/model.hir#L1).
The field-structured serializer follows Serde's
[`serde_derive/src/ser.rs:328–367`](https://github.com/serde-rs/serde/blob/0db9b3b504bcb0a7fa9cf001429869519afd3e9a/serde_derive/src/ser.rs#L328),
and the valid nested-object / missing-field test structure follows serde_json's
[`tests/test.rs:1240–1278`](https://github.com/serde-rs/json/blob/20572a6d066958542d79cdd6cf1a7cd9f0c8e1f8/tests/test.rs#L1240).
Serde and serde_json are MIT OR Apache-2.0. These are structural and test-vector
adaptations; no Rust implementation is copied. Deviations: the smaller DTO uses
two nested records rather than Serde's nested sequence, calls explicit Hirsel
hooks instead of Rust traits, and keeps Hirsel's strict unknown/duplicate-key
policy and typed issue paths. Unicode, ownership and shared-budget checks
extend those vectors. There is no application migration business logic in this
recipe and no change to derive or schema policy.

The field-attributed diagnostic structure and incorrect-converter vectors follow
Serde's [`internals/check.rs:29–53`](https://github.com/serde-rs/serde/blob/0db9b3b504bcb0a7fa9cf001429869519afd3e9a/serde_derive/src/internals/check.rs#L29)
and [`incorrect_type_struct.rs:1–15`](https://github.com/serde-rs/serde/blob/0db9b3b504bcb0a7fa9cf001429869519afd3e9a/test_suite/tests/ui/default-attribute/incorrect_type_struct.rs#L1)
(MIT OR Apache-2.0). No implementation text is copied. Hirsel retains its
explicit hooks, ordinary failure ordering, ownership and authority checking;
diagnostic comments only link generated call errors to existing source spans.
No runtime or replay representation changes.
