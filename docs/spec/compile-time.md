# Compile time

`comptime e` evaluates an ordinary expression while compiling, to initialize a constant or
to produce a value inside a body. Evaluation calls ordinary declared functions, and its
result is materialized data. Static checking still reads only signatures.

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
reflection over source definitions and no derive or body-generation API. The function body
form `= comptime expression` is reserved for generated bodies and is not executable.

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
positions remains forbidden; type/shape inspection remains reserved by CT-3.

### CT-3: Shapes

Reserved for future compile-time features, including opacity-aware type-shape reflection
and typed generation. The reflection type constructor is reserved as `TypeShape[T]`;
`Shape` is an ordinary user type name. No executable shape, build, quotation or
generated-body API exists.
External tools may generate source definitions, which are checked like other source; that
does not replace or exclude future in-language generation.

### CT-4: Order

Compile-time evaluations depend on each other acyclically; a dependency cycle cites its
earliest dependency reference. Step exhaustion reports CT-1 at the enclosing comptime
expression, with the stopping site and budget as notes. The configured default is
1,000,000 TIR steps. **Unspecified:** the optimizer-independent source budget and its
charges. Runtime guards and compile-time call and dependency depth safeguards are
operational tooling outcomes, not comparable language results. Editing the budget
invalidates evaluated caches, so diagnostics retry.

## Reserved for future compile-time features

Generating declarations or function bodies at compile time, reflection over a type's shape,
and compile-time evaluation of callable values are reserved, not excluded. Their APIs,
opacity, ownership and staging contracts are for a future revision of this specification.

- **Syntax.** The `comptime` and `shape` keywords, the `= comptime expression` function
  body, the `shape(T)` primary and the `comptime` prefix stay syntactically reserved where
  unsupported, with no other meaning assigned. LANG-2's rule that expressions carry no type
  arguments describes the current language, not a prohibition on future typed
  compile-time forms. The `file` and `item` productions govern written source, not the kinds
  of checked declaration a future generator may publish.
- **Evaluation and publication.** CT-2's closed frame is the eligibility rule for inline
  expressions only; size inputs are admitted as above. It does not rule out future
  explicit compile-time type, shape or callable inputs. The data-only boundary restricts values
  materialized into a program, not internal callable evaluation or a future typed body or
  declaration artifact. Any future generation publishes declared signatures before
  dependent checking ([LANG-6](language.md#lang-6-signatures),
  [PROG-1](programs.md#prog-1-names)), preserving signature-only checking and nominal
  identity.
