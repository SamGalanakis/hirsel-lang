# Compile time

`comptime e` evaluates an ordinary expression while compiling, to initialize a constant or
to produce a value inside a body. Evaluation calls ordinary declared functions, and its
result is materialized data. Static checking still reads only signatures. Compile-time
code can also read a nominal type's declared fields and variants through `shape(T)`
([CT-3](#ct-3-shapes)), and a function whose header is written in source can take its body
from a compile-time generator ([CT-5](#ct-5-generation)).

```hirsel
fn square(x: u64) -> u64 { x * x }

const TABLE_SIZE: u64 = comptime square(16)

fn scaled(n: u64) -> u64 {
  n * comptime square(4)         // comptime square(n) is CT-2: n is a runtime value
}
```

### CT-1: comptime

`comptime e` evaluates `e` while compiling, to initialize a constant or to produce an
ordinary expression value. Evaluation uses validated ordinary declared calls and
materializes scalar, aggregate, `Map` and `Set` data; dependents check declared signatures.
A failure, an exhausted step limit or a missing capability is a compile error: a well-typed
comptime operation that fails during evaluation is CT-1 at the comptime expression, with the
underlying failure as a note. Static errors inside comptime keep their ordinary owners.
Static checking reads only declared signatures; evaluation executes the selected
implementations and records them as dependencies. There is no implicit build grant, no
reflection over function bodies or source text (CT-3 reads type declarations only).
Generated function bodies are [CT-5](#ct-5-generation); no declaration is generated.

### CT-2: Materialisation

Evaluated values become ordinary constants or expression values before dependents use
them. Only materializable data crosses this boundary: resource, capability and callable
graphs fail CT-2 at the enclosing initializer or explicit comptime expression. An
explicit comptime result must have a data type, checked statically after ordinary
type checking, including in unselected generic definitions. Never and recovery
types do not impose this check; evaluation failures retain CT-1 precedence there. This does not forbid evaluating a
callable internally and publishing its data result. Named constants materialize dependency
first, with ties broken by stable source path and declaration position, and decode once
into immutable storage that lives for the whole program, before entry. Reads share that
value and construct nothing in a noalloc reading frame; startup runs no initializer code.
Finite evaluated data trees have no materialization depth cutoff.

An inline `comptime` expression admits constants, literals, internal `let`s, ordinary
calls and the enclosing instantiation's size parameters. A size-dependent expression
is staged as a private size-generic constant: its signature is published before
selection, and evaluation occurs for each selected size instantiation. Materialized
results are cached by the constant's size actuals and executed dependencies; changing
an executed implementation or the CT-1 budget invalidates the result. Unselected
instantiations do not evaluate. The evaluator, interpreter and native programs use
the same materialized data.

Outer runtime bindings, captures and abstract type names still fail CT-2 at the name,
with the comptime expression as a secondary site. A selected instantiation's failure
keeps CT-1 at the comptime expression and cites the selecting site and size actuals.
Closed expressions inside generic declarations remain legal. Enclosing contracts
are not inherited, but called helpers' contracts apply. Size arithmetic in type
positions remains forbidden, and an abstract type's shape stays a closed-frame failure
([CT-3](#ct-3-shapes)).

### CT-3: Shapes

`shape(T)` is a compile-time value of type `TypeShape[T]`: a token, with no payload of its
own, standing for the declared representation of a nominal record or variant type `T`.
`type_shape.info(s)` reads it as ordinary core data:

```text
ShapeInfo       { name: String, visibility: ShapeVisibility, body: ShapeBody }
ShapeVisibility = Private() | Public() | Opaque()
ShapeBody       = Record(List[FieldShape]) | Choice(List[VariantShape])
FieldShape      { name: String, ty: String }
VariantShape    { name: String, payload: List[PayloadShape] }
PayloadShape    { label: Option[String], ty: String }
```

```hirsel
type User {
  id: u64,
  name: String,
}

fn field_names[T](s: TypeShape[T]) -> List[String] {
  let mut names: List[String] = []
  match type_shape.info(s).body {
    Record(fields) => {
      for f in fields {
        list.push(mut names, copy f.name)
      }
    }
    Choice(_) => (),
  }
  take names
}

const USER_FIELDS: List[String] = comptime field_names(shape(User))
```

- **Placement.** `shape(T)` is written only in compile-time code: inside a `comptime`
  expression or a constant initializer. Elsewhere it is CT-3 at the shape expression. A
  helper's body is run-time code even when only compile-time code calls it, so helpers
  take a `TypeShape[T]` parameter and their callers write `shape(T)`.
- **Subject.** `T` is written at the shape and must be known where it is written: an
  abstract type parameter fails the closed frame of [CT-2](#ct-2-materialisation).
  Eligibility is [GEN-3](generics.md#gen-3-reflection); a hidden representation is
  [PROG-2](programs.md#prog-2-visibility). A transparent alias has its target's shape.
- **Content.** `name` and every `ty` spell a type canonically (below). Fields, variants
  and payloads keep declaration order; a positional payload has no `label`. `visibility`
  is the declaration's own (PROG-2); fields have no separate visibility, so it covers them
  all. Brands, relations, `@growable` markers, bounds, attributes, `uses`, contracts and
  finalizers are not part of a shape.
- **Canonical spelling.** The spelling is valid source in every module: `bool`, `char`,
  sized integers, floats, `String` and `Bytes` by name; tuples `(A, B)` and `(A,)`;
  builtin constructors by name with their arguments (`List[String]`, `Inline[u8, 4]`);
  `box T`; function types as `fn(take A, mut B, C) -> R`, with passing modes and without
  effects; every nominal type, core included, by its full versioned path with its
  arguments (`core@1/Option[u64]`, `app@1/orders.Order`). A generic declaration's
  parameters spell as the actual arguments of the shaped instance.
- **Not data.** A `TypeShape` value never crosses CT-2's materialization boundary: it fails
  CT-2 at the enclosing initializer or comptime expression, as a callable does.
  `ShapeInfo` and its parts are ordinary data and materialize like any other constant.
- **Compile-time-only values.** `TypeShape[T]` and the generated-body type `Body[F]`
  ([CT-5](#ct-5-generation)) are compile-time-only types. A definition whose
  signature mentions one in a parameter or result is a *compile-time-only definition*.
  An expression whose type mentions a compile-time-only type is written only in
  compile-time code or in the body of a compile-time-only definition; elsewhere it is
  CT-3 at the outermost such expression. Run-time code therefore never holds a shape or a
  body, and never calls or names a compile-time-only definition. A non-generic
  compile-time-only definition never runs, so native code omits it; a generic one has
  instances only in compile-time evaluation.
- **Declarations only.** A shape is computed from the type's checked declaration (the owner
  view for an owner), never from a function body or source text. Evaluation records the
  declaration as a compile-time dependency, so editing it, private representation
  included, re-evaluates every reader. `type_shape.info` is a capability-free core
  operation; nothing enumerates a module's declarations, reads attributes or source, or
  reaches the compiler host.

External tools may generate source definitions, which are checked like other source; that
does not replace or exclude in-language generation.

### CT-4: Order

Compile-time evaluations depend on each other acyclically; a dependency cycle cites its
earliest dependency reference. Step exhaustion reports CT-1 at the enclosing comptime
expression, with the stopping site and budget as notes. The configured default is
1,000,000 TIR steps. **Unspecified:** the optimizer-independent source budget and its
charges. Runtime guards and compile-time call and dependency depth safeguards are
operational tooling outcomes, not comparable language results. Editing the budget
invalidates evaluated caches, so diagnostics retry.

### CT-5: Generation

A generated function is an ordinary declaration whose complete header is written in source
and whose body is `= comptime e`. The header (name, generics, labelled parameters with
modes, result, `uses`, `@noalloc`, contracts and attributes) is published exactly like a
written one, before any generator runs, and dependents check against it. Evaluation never
publishes a declaration: no name, signature, type or item comes out of a generator, so every
callable an agent can use is written in source and checking stays signature-only and
independent of topology ([PROG-5](programs.md#prog-5-topology)).

- **Typing.** `e` has type `Result[Body[F], String]`, where `F` is the declared function
  type without uniqueness: its parameter types with their passing modes, result and
  `uses`, with every `unique` dropped. The declaration alone decides uniqueness, since the
  generated body is checked against its header (below), so one generator serves
  `-> Result[T, String]` and `-> Result[unique T, String]`. `Body[F]` is a
  compile-time-only core type ([CT-3](#ct-3-shapes)). Generators are ordinary functions;
  the shaped type's parameter links shape and body, so `record_value(shape(Todo))` with
  `fn record_value[T](s: TypeShape[T]) -> Result[Body[fn(T) -> json.Value uses {}], String]`
  checks from signatures alone as a body for `fn(Todo) -> json.Value uses {}`, and a
  declaration with any other header is LANG-7 at `e` before evaluation. `e` is inside
  CT-2's closed frame: it reads no parameter, and naming a declared parameter in `e` is
  CT-2 at the name. A generic declaration is CT-2 at `e` (generic shape inputs are reserved).
- **Bodies.** `codegen.body(text)` builds a `Body[F]`, taking `F` from the expected type.
  `Error(message)` from a generator is CT-1 at `e`, with the message as a note; a failing
  evaluation is CT-1 as for any comptime expression. The text must be one function literal
  `fn(..) -> .. uses {..} { .. }` and nothing else, or CT-5 at its first token. The literal's
  type must equal the declared function type up to uniqueness, or LANG-7 at its header. Its
  parameters bind the declared parameters by position: the body uses the literal's names,
  and diagnostics in the body name them at the literal's parameters, while the header and
  contracts keep the declared ones. Shapes spell types canonically, so a generator can write parameter and
  field types that resolve the same way in every module.
- **Checking.** A generated body is checked exactly as if written in the declaring module:
  that module's names, package view and PROG-2 opacity, against the declared header, under
  every ownership, mode, authority, allocation and contract rule, so a `-> unique T`
  header requires a body that builds a fresh value (MODE-2). A client package cannot
  reach an opaque representation through generated text. Diagnostics keep their ordinary
  rule owners; the primary span is in the generated text and a secondary label marks `e`.
  Generated bodies construct and read values only through ordinary source, so nominal
  identities, explicit capability parameters and allocation sites stay visible in them.
  `hirsel expand <file>` prints each generated body of a source file as source.
- **Staging.** For each generated definition: its header is published; dependents check
  against it; the generator call is checked from signatures and evaluated with no
  capability and the CT-4 budget; the resulting text is cached under the implementations it
  executed and the declarations its shapes read ([CT-2](#ct-2-materialisation)); then the
  generated body is checked and lowered like any body. The same inputs give the same text.
  Generation that needs its own result is a CT-4 cycle at `e`.

**Worked example.** `examples/todo-api` derives its `todo_value` with
[`std@1/derive.json_value`](std-library.md#derived-bodies); this is that generator reduced to
three field types:

```text
type Todo { id: u64, title: String, done: bool }

fn todo_value(t: Todo) -> json.Value uses {} = comptime record_value(shape(Todo))

fn record_value[T](s: TypeShape[T]) -> Result[Body[fn(T) -> json.Value uses {}], String] {
  let info = type_shape.info(s)
  let Record(fields) = info.body else { return Error("record_value needs a record") }
  let mut lines: List[String] = [
    string.concat(string.concat("fn(value: ", info.name), ") -> std@1/json.Value uses {} {"),
    "  let mut fields: Map[String, std@1/json.Value] = map.empty()",
  ]
  for f in fields {
    let read = string.concat("value.", f.name)
    let value = match f.ty {
      "u64" => string.join(["Number(std@1/u64.to_string(", read, "))"], ""),
      "String" => string.join(["Text(unique ", read, ")"], ""),
      "bool" => string.join(["Bool(", read, ")"], ""),
      _ => return Error(string.concat("no JSON form for ", f.ty)),
    }
    list.push(mut lines, string.join(["  map.insert(mut fields, \"", f.name, "\", ", value, ")"], ""))
  }
  list.push(mut lines, "  Object(take fields)")
  list.push(mut lines, "}")
  Ok(codegen.body(string.join(lines, "\n")))
}
```

It generates, and the compiler then checks as `todo_value`'s body:

```text
fn(value: app@1/Todo) -> std@1/json.Value uses {} {
  let mut fields: Map[String, std@1/json.Value] = map.empty()
  map.insert(mut fields, "id", Number(std@1/u64.to_string(value.id)))
  map.insert(mut fields, "title", Text(unique value.title))
  map.insert(mut fields, "done", Bool(value.done))
  Object(take fields)
}
```

Adding a field to `Todo` re-evaluates the generator; a field type it has no form for fails
CT-1 at `todo_value`'s `= comptime` expression, naming the type. Field naming, omission and domain policy stay explicit
generator code; nothing is derived implicitly.

## Reserved for future compile-time features

Generating declarations at compile time and compile-time evaluation of callable values are
reserved, not excluded. Their APIs, ownership and staging contracts are for a future
revision of this specification. Generated function bodies are [CT-5](#ct-5-generation).

- **Syntax.** The `comptime` keyword and the `comptime` prefix stay syntactically reserved
  where unsupported, with no other meaning
  assigned. LANG-2's rule that expressions carry no type arguments describes the current
  language, not a prohibition on future typed compile-time forms. The `file` and `item`
  productions govern written source, not the kinds of checked declaration a future
  generator may publish.
- **Evaluation and publication.** CT-2's closed frame is the eligibility rule for inline
  expressions only; size inputs are admitted as above. It does not rule out future
  explicit compile-time type, shape or callable inputs. The data-only boundary restricts values
  materialized into a program, not internal callable evaluation or a future typed body or
  declaration artifact. Any future generation publishes declared signatures before
  dependent checking ([LANG-6](language.md#lang-6-signatures),
  [PROG-1](programs.md#prog-1-names)), preserving signature-only checking and nominal
  identity.

## Rationale: shapes and generation

**Decision.** Compile-time code reads a type's declaration as data through a phantom-typed
token, and generation (when it lands) emits ordinary bodies, as text, for headers written in
source. Three properties drive it. Checking stays signature-only: dependents never wait on
evaluation, because evaluation produces neither signatures nor declarations. Opacity is a
property of the reader's package, enforced where `shape(T)` is written, so a shape can never
reveal more than source in the same module could name. And the core IR does not grow:
`shape(T)` is the existing TIR shape rvalue, `type_shape.info` is an ordinary core call
evaluated by the compile-time interpreter, and its result is ordinary materialized data.

**Alternatives considered.**

- *Zig `@typeInfo` and comptime type construction.* Zig hands compile-time code a complete
  `std.lang.Type` for any type, with every field in source order, its declarations and its
  function types, and builds new types with `@Struct`, `@Union` and `@Enum`; `inline for`
  then specializes a body per field. It is the most expressive design and the model for
  reading fields as data. Hirsel rejects the rest: Zig fields carry no visibility, so any
  code that can name a type reads its whole representation and opacity would become
  advisory; constructed types and `inline for` make a declaration's meaning depend on
  evaluation, which contradicts signature-only checking; and `type` values would need a new
  kind of compile-time value in the IR. Hirsel keeps the read, as data, and leaves out the
  construction.
- *Rust derive macros.* A derive runs a procedural macro over the declaration's tokens and
  emits items. It is opt-in and keeps generated code reviewable, but the macro sees tokens,
  not checked types, so it cannot tell an alias from its target or a hidden field from a
  visible one; it can emit arbitrary items, including signatures, so dependents wait on
  expansion; and it runs as compiler-host code with ambient file and process access. Hirsel
  takes the opt-in, per-declaration generation site and rejects token input, item output and
  host access.
- *`go generate`.* An explicitly invoked external command writes ordinary source that is
  checked like any other. It is simple and fully visible, and it stays available
  ([CT-3](#ct-3-shapes)). It is not the in-language answer because its output goes stale
  silently when a type changes, it needs a toolchain outside the compiler, and it bypasses
  the dependency tracking that re-evaluates a shape reader after an edit.
- *Typed syntax builders instead of text.* A quotation or AST API would make malformed
  output unrepresentable, at the cost of a second, larger surface for every syntactic form.
  Generated text is checked in full anyway, with diagnostics in the generated source, and
  agents already write Hirsel source well; one way per job favours the text.
- *Shape as plain data with no token.* `shape(T)` could return `ShapeInfo` directly. The
  `TypeShape[T]` token keeps the type in the static world, which is what lets a generic
  generator's signature promise a body for `fn(T) -> R` and lets the checker reject a
  mismatched header before evaluation.
