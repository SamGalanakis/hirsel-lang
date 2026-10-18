# Hirsel in one page

Browse the [full signature index](api.md) for every public core and std declaration.

Start here when writing Hirsel. The [specification](spec/index.md) defines the
rules; [building](building.md) covers toolchains and launch details. The programs
below are small enough to copy and run from the repository root with
`hirsel run <linked-source.hir>`. Each selects
`std@1` through its adjacent `.hir.json` manifest; copy that sidecar too, and
adjust its source and dependency paths when moving a program. There are no imports
or dependency downloads. `json.parse` names a selected package definition;
`out.print` is supplied by `core@1`.

## Statements at a glance

Use `while`, `for x in xs`, half-open ranges `0u64..n`, `break`, `continue`,
and checked indexing `xs[i]`; assign with `xs[i] = value` on a mutable list.

<!-- tour: conformance/LANG/LANG-6-tour-00-statements-run.hir -->
[Runnable source](../conformance/LANG/LANG-6-tour-00-statements-run.hir)

```hirsel
pub fn main() -> Result[(), ()] {
  let mut xs: List[u64] = [1, 2, 3]
  xs[0] = 4
  let mut total = 0u64
  for x in xs {
    total += x
  }
  for i in 0u64..list.len(xs) {
    if i == 1 {
      continue
    }
    total += xs[i]
  }
  while total < 20 {
    total += 1
    if total == 19 {
      break
    }
  }
  failure.check(total == 19, Arithmetic())
  Ok(())
}
```

Expected:

No stdout; exit 0 (all checks pass).
<!-- /tour -->

Use `'outer: for ...` or `'outer: while ...` to name an enclosing loop.
`break 'outer` leaves it; `continue 'outer` advances it, cleaning up the
inner scopes on the way. Labels are lexical within one function, cannot
shadow an active label, and carry no break value.

<!-- tour: conformance/LANG/LANG-11-tour-loop-labels-run.hir -->
[Runnable source](../conformance/LANG/LANG-11-tour-loop-labels-run.hir)

```hirsel
pub fn main() -> Result[(), String] {
  let rows: List[List[u64]] = [[1, 2], [3, 7], [8, 9]]
  let mut row: u64 = 0
  'search: for i in 0u64..list.len(rows) {
    for value in rows[i] {
      if value == 7 {
        row = i
        break 'search
      }
    }
  }
  failure.check(row == 1, TextInvariant())
  Ok(())
}
```

Expected:

No stdout; exit 0 (all checks pass).
<!-- /tour -->

To read one decimal number, pass the line returned by `input.read_line` to
`u64.parse(string.trim(line))`. Matching the parse result handles invalid input;
`?` propagates `scalar.ParseError`.

<!-- tour: conformance/LANG/LANG-8-tour-00-number-run.hir -->
[Runnable source](../conformance/LANG/LANG-8-tour-00-number-run.hir)

```hirsel
pub fn main() -> Result[(), scalar.ParseError] {
  let line = " 42\n"
  let n = u64.parse(string.trim(line))?
  failure.check(n == 42, Arithmetic())
  Ok(())
}
```

Expected:

No stdout; exit 0 (all checks pass).
<!-- /tour -->

To run against scratch files, mount the scratch `data/` directory explicitly.
From the repository root (replace the source with your Io entry):

```sh
scratch=$(mktemp -d)
mkdir "$scratch/data"
printf '{"io":{"dirs":[{"path":"data","host":"%s/data","access":"read_write"}]}}\n' "$scratch" > "$scratch/grants.json"
hirsel run program.hir --grants "$scratch/grants.json"
```

`--grants` and `--run` take inline JSON when the argument starts with `{`,
otherwise a path. For example, grant only stdout with
`hirsel run program.hir --grants '{"io":{"stdout":"stdout.bytes@1","dirs":[]}}'`.
Built executables use `--hirsel-grants` and `--hirsel-run` so the program keeps
its own flag space. Inline directory grants need absolute host paths.
`hirsel run program.hir -- 200 7` grants only Args with those exact values;
an entry also requiring Io needs an explicit Io grant.

Acquire that mount with `io.dir(rt, path.of("data"))`; file paths are relative
to the mount. Use `read` access when the program only reads.

The mistakes to avoid first:

- Give numeric literals a context: `let n = 3u64` or `let n: u64 = 3`.
  `let n = 3` has no default integer type (LANG-2). Type arguments belong in
  types, never calls: `let xs: List[u64] = []`, then `list.push(mut xs, 1)`.
- Every top-level function has a complete signature. An omitted result means
  `()`, not an inferred result. Use the last expression for a block's value;
  a final `return` is rejected (LANG-6, LANG-11).
- Functions are module-qualified, not methods. Build records with labelled
  fields; write variants bare, such as `Ok(())`, never `Result.Ok(())`.
  Match refutable patterns, test one with `if let P = e { .. }` or
  `while let`, or bind one with `let P = e else { return .. }` (LANG-4, LANG-12,
  LANG-13).
- Reading a parameter borrows it. `let y = x` moves counted data;
  `copy x` shares it; `unique x` deep-copies when necessary. `copy` does not
  extend local lifetimes. Match `mut` and `take` at the declaration and call.
- A normal `?` needs the same error type. Use `? Variant` to wrap another
  error type. Integer overflow and failed assertions are language failures,
  not `Result` errors caught by `?` (LANG-8, SEM-1, FAIL-1).
- Selecting code grants no I/O. Pass capabilities and supply launch grants.
  A `Path` is a name, not filesystem authority (CAP-1, PROG-3).

## Values and types

Records, variants, tuples and sized scalars have value semantics. `match` is
exhaustive, and arithmetic uses one integer width at a time. Overflow fails;
choose `int.checked_add` or `int.wrapping_add` explicitly when needed. A public
signature may mention only public types.

Run `hirsel run conformance/LANG/LANG-2-tour-01-values-run.hir`.
The default source runner uses a simulated world; it grants no host access.

<!-- tour: conformance/LANG/LANG-2-tour-01-values-run.hir -->
[Runnable source](../conformance/LANG/LANG-2-tour-01-values-run.hir)

```hirsel
type Point {
  x: u64,
  y: u64,
}

type Figure = Dot(Point) | Empty

pub type AppError = Grant(GrantError) | Output(OutputError)

fn score(item: Figure) -> u64 {
  match item {
    Dot(p) => p.x + p.y,
    Empty() => 0,
  }
}

pub fn main(rt: Io) -> Result[(), AppError] {
  let p = Point(x: 3, y: 4)
  let n = score(Dot(p))
  let output = io.stdout(rt) ? Grant
  out.print(output, u64.to_string(n)) ? Output
  out.print(output, "\n") ? Output
  Ok(())
}
```

Expected:

```text
7
```
<!-- /tour -->

## Ownership and modes in practice

A borrowed parameter is read-only. `mut` changes caller storage exclusively;
`take` transfers ownership on both success and error. Copying a list preserves
independent value semantics: mutation detaches shared storage as needed. The
program keeps `xs`, changes `ys`, then moves `ys` into `keep`.

A counted result derived from a borrowed parameter needs a relation, for example
`fn view(xs: List[u64]) -> List[u64] in xs { xs }`. Its live result freezes `xs`
until last use. Returning it without `in xs` is MODE-1; adding `copy` cannot fix
that. A plain scalar result, like `first` below, is independent. Use
`fn keep(take xs: List[u64]) -> List[u64]` when ownership should leave the caller.

<!-- tour: conformance/OWN/OWN-4-tour-02-modes-run.hir -->
[Runnable source](../conformance/OWN/OWN-4-tour-02-modes-run.hir)

```hirsel
pub type AppError = Grant(GrantError) | Output(OutputError)

fn grow(mut xs: List[u64]) {
  list.push(mut xs, 3)
}

fn keep(take xs: List[u64]) -> List[u64] {
  take xs
}

fn first(xs: List[u64]) -> u64 {
  xs[0]
}

pub fn main(rt: Io) -> Result[(), AppError] {
  let xs: List[u64] = [1, 2]
  let mut ys = copy xs
  ys[0] = 9
  grow(mut ys)
  let kept = keep(take ys)
  let output = io.stdout(rt) ? Grant
  out.print(output, u64.to_string(first(xs))) ? Output
  out.print(output, "\n") ? Output
  out.print(output, u64.to_string(list.len(kept))) ? Output
  out.print(output, "\n") ? Output
  Ok(())
}
```

Expected:

```text
1
3
```
<!-- /tour -->

## Errors and variant mappers

Expected failures are ordinary `Result[T, E]` values. `parse(text) ? Input`
wraps its `ParseError` in `AppError.Input`, then propagates. Plain `?` performs
no implicit conversion. A mapper may also be a named one-argument borrowed or
taken function; it cannot require `mut`. Every early exit still cleans up owned
resources. An entry returning `Error` exits 1 and prints nothing implicitly.

<!-- tour: conformance/LANG/LANG-8-tour-03-errors-run.hir -->
[Runnable source](../conformance/LANG/LANG-8-tour-03-errors-run.hir)

```hirsel
pub type ParseError = Empty | TooLong(u64)

pub type AppError = Input(ParseError) | Grant(GrantError) | Output(OutputError)

fn parse(text: String) -> Result[u64, ParseError] {
  if string.byte_len(text) == 0 {
    return Error(Empty())
  }
  Ok(string.byte_len(text))
}

fn count(text: String) -> Result[u64, AppError] {
  let n = parse(text) ? Input
  Ok(n + 1)
}

pub fn main(rt: Io) -> Result[(), AppError] {
  let message = match count("") {
    Ok(_) => "success\n",
    Error(Input(Empty())) => "input error\n",
    Error(_) => "other error\n",
  }
  out.print(io.stdout(rt) ? Grant, message) ? Output
  Ok(())
}
```

Expected:

```text
input error
```
<!-- /tour -->

## Generic functions as callbacks

An annotated callable type can specialize a top-level generic function name. The same
works for arguments, record fields and returns when the complete expected callable type
determines every type and size parameter. The declared modes and effects still apply;
missing context or an undetermined parameter is LANG-2.

<!-- tour: conformance/LANG/LANG-2-generic-callable-context-run.hir -->
[Runnable source](../conformance/LANG/LANG-2-generic-callable-context-run.hir)

```hirsel
fn identity[T](take value: T) -> T {
  value
}

fn apply(f: fn(take u64) -> u64 uses {}, value: u64) -> u64 {
  f(value)
}

type Callback {
  call: fn(take u64) -> u64 uses {},
}

fn callback() -> fn(take u64) -> u64 uses {} {
  identity
}

fn returned(early: bool) -> fn(take u64) -> u64 uses {} {
  if early {
    return identity
  }
  identity
}

pub fn main() -> Result[(), String] {
  let f: fn(take u64) -> u64 uses {} = identity
  let record = Callback(call: identity)
  failure.check(f(3) == 3, Bounds())
  failure.check(apply(identity, 5) == 5, Bounds())
  failure.check(record.call(7) == 7, Bounds())
  let length: fn(List[u64]) -> u64 uses {} = list.len
  let values: List[u64] = [17u64, 19u64]
  failure.check(length(values) == 2, Bounds())
  let g = callback()
  let h = returned(true)
  failure.check(g(11) == 11 && h(13) == 13, Bounds())
  Ok(())
}
```

Expected:

No stdout; exit 0 (all checks pass).
<!-- /tour -->

## Option and Result helpers

Use `option.remove(mut value)` to move out the current option and leave `None`.
A unique payload remains unique.

Use `option.is_some` / `is_none` and `results.is_ok` / `is_error` to inspect a
borrowed value. Both modules provide `map`, `and_then`, `or_else`, `flatten`,
`unwrap_or` (eager fallback) and `unwrap_or_else` (lazy fallback). `option.filter`
borrows its payload for the predicate; `ok_or` / `ok_or_else` turn absence into an
error, and `zip` pairs present values. `results.map_error` transforms an error;
`results.ok` / `error` keep one branch as an option. Transformations take their
inputs and callbacks; callbacks may consume their captures once. Hirsel has no panicking `unwrap` or `expect`.

<!-- tour: conformance/LANG/LANG-8-tour-option-result-run.hir -->
[Runnable source](../conformance/LANG/LANG-8-tour-option-result-run.hir)

```hirsel
pub fn main() -> Result[(), ()] {
  let present: Option[u64] = Some(21)
  let doubled = option.map(
    take present,
    fn(take n: u64) -> u64 {
      n * 2
    }
  )
  failure.check(option.unwrap_or(take doubled, 0u64) == 42, Bounds())
  let missing: Option[u64] = None
  let value: Result[u64, String] = option.ok_or(take missing, "missing")
  let recovered = results.unwrap_or_else(
    take value,
    fn(take error: String) -> u64 {
      failure.check(error == "missing", Bounds())
      7
    }
  )
  failure.check(recovered == 7, Bounds())
  Ok(())
}
```

Expected:

No stdout; exit 0 (all checks pass).
<!-- /tour -->

## Rejecting invalid input

Use `Result[(), String]` for an entry that needs to fail with an application
message. `Error("bad input: ...")` exits 1; no fake `FileFault(0u32)` is needed.
A helper can return `Result[T, String]`, and plain `?` propagates its message.
The line below can come from `input.read_line`; the example deliberately rejects it.

<!-- tour: conformance/LANG/LANG-8-tour-12-bad-input-run.hir -->
[Runnable source](../conformance/LANG/LANG-8-tour-12-bad-input-run.hir)

```hirsel
pub fn main() -> Result[(), String] {
  let line = "not a number"
  let number = match u64.parse(string.trim(line)) {
    Ok(n) => n,
    Error(_) => return Error("bad input: expected one decimal number"),
  }
  failure.check(number == 42, Arithmetic())
  Ok(())
}
```

Expected:

No stdout; exit 1.
<!-- /tour -->

The same parser accepts a trimmed decimal:

<!-- tour: conformance/LANG/LANG-8-tour-12-valid-input-run.hir -->
[Runnable source](../conformance/LANG/LANG-8-tour-12-valid-input-run.hir)

```hirsel
pub fn main() -> Result[(), String] {
  let line = " 42\n"
  let number = match u64.parse(string.trim(line)) {
    Ok(n) => n,
    Error(_) => return Error("bad input: expected one decimal number"),
  }
  failure.check(number == 42, Arithmetic())
  Ok(())
}
```

Expected:

No stdout; exit 0 (all checks pass).
<!-- /tour -->

## Patterns: conditional let, alternatives and ranges

`let P = e else { ... }` binds a refutable pattern into the enclosing block;
the else block must diverge with `return`, `break`, `continue` or a call that
never returns. `if let P = e { ... } else { ... }` runs its block with the
bindings when `e` matches, and `while let P = e { ... }` repeats while it does,
evaluating `e` before each iteration. All three borrow or take exactly as
`match` does, and there are no let chains (`if let .. && ..`). `P | Q` matches
either alternative wherever a pattern may appear; every alternative binds the
same names with the same types. `a..=b` matches a closed integer, `char` or
byte range with literal bounds, and ranges count toward exhaustiveness, so
`0..=127` and `128..=255` cover `u8` without a wildcard. There is no
half-open range pattern (LANG-13).

<!-- tour: conformance/LANG/LANG-13-tour-patterns-run.hir -->
[Runnable source](../conformance/LANG/LANG-13-tour-patterns-run.hir)

```hirsel
fn class(c: char) -> u64 {
  match c {
    'a'..='z' | 'A'..='Z' | '_' => 1,
    '0'..='9' => 2,
    ' ' | '\t' => 3,
    _ => 0,
  }
}

fn ascii(byte: u8) -> bool {
  match byte {
    0..=127 => true,
    128..=255 => false,
  }
}

fn number(line: String) -> Result[u64, String] {
  let Ok(n) = u64.parse(string.trim(line)) else {
    return Error("bad input: expected one decimal number")
  }
  Ok(n)
}

pub fn main() -> Result[(), String] {
  failure.check(class('q') == 1 && class('7') == 2 && class('\t') == 3, TextInvariant())
  failure.check(ascii(0x41) && !ascii(0xE9), TextInvariant())
  let n = number(" 42\n")?
  failure.check(n == 42, Arithmetic())
  let words: List[String] = ["3", "x", "4"]
  let mut total = 0u64
  for word in words {
    let Ok(v) = u64.parse(word) else {
      continue
    }
    total += v
  }
  failure.check(total == 7, Arithmetic())
  let mut queue: unique List[u64] = [5, 6]
  while let Some(v) = list.pop(mut queue) {
    total += v
  }
  if let Ok(v) = u64.parse("8") {
    total += v
  }
  failure.check(total == 26, Arithmetic())
  Ok(())
}
```

Expected:

No stdout; exit 0 (all checks pass).
<!-- /tour -->

## Collections

`List[T]` grows, `Array[T]` is a heap sequence, and `Inline[T, n]` has its size in
its type. Indexes are checked. Borrowed loops read elements, `for x in mut xs`
updates element places, and `for x in take xs` consumes the collection.

Maps iterate in insertion order; replacement preserves a key's position.
`map.get` returns `Option[V] in m`. Map equality is order-sensitive;
`map.same_entries` ignores insertion order. Sets ignore order for equality.
`std@1/list` provides sorting, filtering and folds with explicit callbacks.

<!-- tour: conformance/SEM/SEM-5-tour-04-collections-run.hir -->
[Runnable source](../conformance/SEM/SEM-5-tour-04-collections-run.hir)

```hirsel
pub type AppError = Grant(GrantError) | Output(OutputError)

pub fn main(rt: Io) -> Result[(), AppError] {
  let mut xs: List[u64] = [1, 2]
  for x in mut xs {
    x += 1
  }
  let mut counts: Map[String, u64] = map.empty()
  map.insert(mut counts, "pear", xs[0])
  map.insert(mut counts, "apple", xs[1])
  map.insert(mut counts, "pear", 4)
  let mut seen: Set[String] = set.empty()
  let output = io.stdout(rt) ? Grant
  for (key, value) in counts {
    if !set.contains(seen, key) {
      out.print(output, key) ? Output
      out.print(output, "\n") ? Output
      set.insert(mut seen, unique key)
    }
  }
  Ok(())
}
```

Expected:

```text
pear
apple
```
<!-- /tour -->

## Strings and bytes

Raw literals keep quotes, backslashes and physical LF newlines as data:
`r#"{"path":"C:\tmp"}"#` is a String and `br"a\nb"` contains a literal
backslash followed by `n`. Add hashes around the quotes when the body contains
quotes; the closing delimiter must match. Multiline bodies retain their exact
indentation. CRLF inside a raw literal is rejected. Raw literals have no
interpolation or escapes.

`String` is valid UTF-8; `Bytes` may contain arbitrary bytes. There is no generic
string length or integer string indexing. Choose byte length, Unicode scalar
iteration or `unicode.graphemes`. A `StrPos of text` comes from that exact
string and permits a boundary-safe slice. Position 1 is inside `é` and would
return `None()`. Slices keep their owner alive and retain its locality.
`string.from_bytes(take data)` returns the original bytes on invalid UTF-8.

When a consuming string operation follows a borrowed view, make ownership explicit:
`words` below borrows its input for `trim`, then uses `unique` to make the owned
string required by `split`. `copy` would only share the borrowed blocks and keep
their locality; it would not satisfy the taken global input.

<!-- tour: conformance/SEM/SEM-3-tour-05-text-run.hir -->
[Runnable source](../conformance/SEM/SEM-3-tour-05-text-run.hir)

```hirsel
pub type AppError = Boundary | Grant(GrantError) | Output(OutputError)

fn words(line: String) -> List[String] {
  string.split(unique string.trim(line), " ")
}

pub fn main(rt: Io) -> Result[(), AppError] {
  failure.check(words("  é sheep  ") == ["é", "sheep"], TextInvariant())
  let text = "é"
  let start = match string.position(text, 0) {
    Some(p) => p,
    None() => return Error(Boundary()),
  }
  let end = match string.position(text, 2) {
    Some(p) => p,
    None() => return Error(Boundary()),
  }
  let part = string.slice(text, start, end)
  let mut scalars: u64 = 0
  for scalar in string.scalars(part) {
    scalars += 1
  }
  let raw: Bytes = b"\xff"
  let invalid = string.from_bytes(copy raw)
  failure.check(invalid == Error(raw), TextInvariant())
  let output = io.stdout(rt) ? Grant
  out.print(output, part) ? Output
  out.print(output, "\n") ? Output
  out.print(output, u64.to_string(string.byte_len(part))) ? Output
  out.print(output, "\n") ? Output
  out.print(output, u64.to_string(scalars)) ? Output
  out.print(output, "\n") ? Output
  Ok(())
}
```

Expected:

```text
é
2
1
```
<!-- /tour -->

Text lookup: `string.to_upper` / `to_lower` perform full Unicode16 casing
(including expansions and final sigma); `string.casefold` / `eq_ignore_case`
use full default case folding. `unicode.normalize_nfc` / `normalize_nfd` /
`normalize_nfkc` / `normalize_nfkd` normalize explicitly. These return owned
text; folding alone does not normalize. The `ascii_lower` / `ascii_upper`
variants and scalar `unicode.simple_lower` / `simple_upper` keep their meanings.

## Characters

`char` is one Unicode scalar value: 0 through 0x10FFFF without the surrogates.
A character literal such as `'a'` or `'\u{1f411}'` has type `char`;
`string.scalars` yields `char` and the `unicode.*` predicates take it.
A `char` is not an integer: `char.to_u32` gives its scalar value and
`char.from_u32` returns `None()` for a surrogate or a value above 0x10FFFF.
`char.format_into` appends the UTF-8 encoding, and an f-string hole of type
`char` prints the character. Bytes stay `u8`: a byte literal such as `b','`
is one ASCII byte and compares with an element of `Bytes`.

<!-- tour: conformance/SEM/SEM-3-tour-05-chars-run.hir -->
[Runnable source](../conformance/SEM/SEM-3-tour-05-chars-run.hir)

```hirsel
pub type AppError = Grant(GrantError) | Output(OutputError)

fn initials(text: String) -> String {
  let mut out: List[u8] = []
  for c in string.scalars(text) {
    if c == ' ' {
      char.format_into('-', mut out)
    } else {
      char.format_into(c, mut out)
    }
  }
  text.finish_valid(take out)
}

pub fn main(rt: Io) -> Result[(), AppError] {
  let mut upper: u64 = 0
  let mut letters: u64 = 0
  for c in string.scalars("S heep") {
    if unicode.simple_upper(c) == c && unicode.is_letter(c) {
      upper += 1
    }
    if unicode.is_letter(c) {
      letters += 1
    }
  }
  let sheep = '🐑'
  let data = b"x,y"
  failure.check(data[1] == b',' && char.from_u32(0xd800) == None(), TextInvariant())
  let name = initials("S heep")
  let output = io.stdout(rt) ? Grant
  out.print(output, f"{name}: {upper} upper, {letters} letters\n") ? Output
  out.print(output, f"{sheep} is U+{char.to_u32(sheep):x}\n") ? Output
  Ok(())
}
```

Expected:

```text
S-heep: 1 upper, 5 letters
🐑 is U+1f411
```
<!-- /tour -->

## Building text with f-strings

An `f`-prefixed string builds a fresh `unique String` from its text and the values
in its `{holes}`. A hole holds any expression, including calls and string literals;
`String`, `bool`, the integers and the floats format as their `to_string` text, a
type that declares `@format` formats through its function (next section), and any
other type is a compile error naming it.
`{{` and `}}` are literal braces. Holes are borrowed reads, and an f-string needs
`std@1` selected. Format specifiers follow the expression: `{n:08}` zero-pads a number,
`{mean:.2}` rounds a float to two decimal places, `{name:*^12}` centers text,
and `{n:#x}` includes the hex prefix.

<!-- tour: conformance/SEM/SEM-3-tour-14-fstrings-run.hir -->
[Runnable source](../conformance/SEM/SEM-3-tour-14-fstrings-run.hir)

```hirsel
pub type AppError = Grant(GrantError) | Output(OutputError)

type Chore {
  name: String,
  done: bool,
}

fn summary(chores: List[Chore]) -> String {
  let mut done = 0u64
  for chore in chores {
    if chore.done {
      done += 1
    }
  }
  let total = list.len(chores)
  f"{done:1} of {total} chores done ({done * 100u64 / total}%)\n"
}

pub fn main(rt: Io) -> Result[(), AppError] {
  let chores: List[Chore] = [
    Chore(name: "plan", done: true),
    Chore(name: "build", done: true),
    Chore(name: "test", done: true),
    Chore(name: "write docs", done: false)
  ]
  let output = io.stdout(rt) ? Grant
  out.print(output, summary(chores)) ? Output
  out.print(output, f"next: {chores[3].name}, {{braces}} stay literal\n") ? Output
  let mean = 10.0f64 / 4.0f64
  out.print(output, f"mean {mean:.1}, all done: {chores[3].done}\n") ? Output
  Ok(())
}
```

Expected:

```text
3 of 4 chores done (75%)
next: write docs, {braces} stay literal
mean 2.5, all done: false
```
<!-- /tour -->

## Formatting your own types

A type joins f-strings by naming one function with `@format`. The function borrows
the value and appends its text to a `mut out: List[u8]` builder, usually with another
f-string. The association promises that the function keeps the builder unique;
the compiler proves its body, so no ownership annotation is needed. A hole of
that type calls it, so records, variants and generic types (with a function generic
over the same parameters) format like scalars; nothing formats without the
attribute. Fill, alignment and width pad the produced text as they pad strings.
A width or precision can come from a `u64` in scope: `{name:width$}`, `{hours:.places$}`.
Text width measures estimated display cells over whole Unicode16 grapheme clusters:
`text.display_width("界")` is 2 and `text.display_width("e\u{301}")` is 1.
Thus `f"|{"界":>4}|"` produces `"|  界|"`, and precision 1 keeps all of
`"e\u{301}"`, including its accent. String precision keeps the longest whole-cluster
prefix that fits. A wide fill repeats only while it fits each side's padding,
then spaces fill the remaining cell: `f"|{"x":界>4}|"` is `"|界 x|"`.

<!-- tour: conformance/SEM/SEM-3-tour-15-format-run.hir -->
[Runnable source](../conformance/SEM/SEM-3-tour-15-format-run.hir)

```hirsel
pub type AppError = Grant(GrantError) | Output(OutputError)

@format(ticket_text)
type Ticket {
  id: u64,
  done: bool,
}

fn ticket_text(ticket: Ticket, mut out: List[u8]) -> () uses {} {
  let state = if ticket.done {
    "done"
  } else {
    "open"
  }
  text.append(mut out, f"#{ticket.id} {state}")
}

pub fn main(rt: Io) -> Result[(), AppError] {
  let output = io.stdout(rt) ? Grant
  let tickets: List[(Ticket, String)] = [
    (Ticket(id: 7, done: false), "ada"),
    (Ticket(id: 12, done: true), "grace")
  ]
  let width: u64 = 9
  out.print(output, f"| {"ticket":width$} | {"owner":width$} |\n") ? Output
  for (ticket, owner) in tickets {
    out.print(output, f"| {ticket:width$} | {owner:width$} |\n") ? Output
  }
  let hours = 2.5f64
  let places: u64 = 2
  out.print(output, f"total {hours:.places$}h\n") ? Output
  Ok(())
}
```

Expected:

```text
| ticket    | owner     |
| #7 open   | ada       |
| #12 done  | grace     |
total 2.50h
```
<!-- /tour -->

## Standard library: JSON

`std@1` is ordinary Hirsel source. JSON numbers retain their decimal spelling;
objects retain insertion order and duplicate keys are rejected. There is no
automatic conversion of application records to JSON: construct or inspect
`json.Value`, whose nested arrays and objects contain `Value`s.

<!-- tour: conformance/LANG/LANG-8-tour-06-json-run.hir -->
[Runnable source](../conformance/LANG/LANG-8-tour-06-json-run.hir)

```hirsel
pub type AppError = Json(json.JsonError) | Grant(GrantError) | Output(OutputError)

pub fn main(rt: Io) -> Result[(), AppError] {
  let value = json.parse("{\"n\": 1, \"ok\": true}") ? Json
  let text = json.stringify(value) ? Json
  out.print(io.stdout(rt) ? Grant, text) ? Output
  out.print(io.stdout(rt) ? Grant, "\n") ? Output
  Ok(())
}
```

Expected:

```text
{"n":1,"ok":true}
```
<!-- /tour -->

## Standard library: paths and time

Path normalization and RFC 3339 parsing are pure algorithms. Normalizing a path
does not grant access or resolve symlinks. `time.Duration` is the one duration
type: signed nanoseconds, used for formatting, parsing, `time.between` and
`time.sleep`. Sampling or waiting on real time needs `io.monotonic_clock` or
`io.wall_clock` and its explicit grant. An `Instant of clock` belongs to that clock; time sampling is
never an ambient pure operation.

<!-- tour: conformance/LANG/LANG-8-tour-07-path-time-run.hir -->
[Runnable source](../conformance/LANG/LANG-8-tour-07-path-time-run.hir)

```hirsel
pub type AppError = PathError(path.PathError) | Parse(scalar.ParseError) | Time(
  time.TimeError
) | Grant(GrantError) | Output(OutputError)

pub fn main(rt: Io) -> Result[(), AppError] {
  let p = path.normalize(path.of("notes/./draft/../tour.txt")) ? PathError
  let stamp = time.parse("1970-01-01T01:00:00+01:00") ? Parse
  let text = time.format(stamp) ? Time
  let duration = duration.parse("1.500s") ? Parse
  let output = io.stdout(rt) ? Grant
  out.print(output, path.text(p)) ? Output
  out.print(output, "\n") ? Output
  out.print(output, text) ? Output
  out.print(output, "\n") ? Output
  out.print(output, duration.format(duration)) ? Output
  out.print(output, "\n") ? Output
  Ok(())
}
```

Expected:

```text
notes/tour.txt
1970-01-01T00:00:00Z
1.5s
```
<!-- /tour -->

## Compile-time data with generic sizes

`comptime` can compute data from a selected function instantiation's size parameters.
Each selected size is evaluated while compiling and cached with the implementations
it executes. Runtime arguments and abstract type parameters cannot be comptime inputs;
size arithmetic in type arguments remains forbidden.

<!-- tour: conformance/CT/CT-2-closed-frame-size-run.hir -->
[Runnable source](../conformance/CT/CT-2-closed-frame-size-run.hir)

```hirsel
fn square(n: u64) -> u64 {
  n * n
}

fn scaled[n: size](xs: Inline[u8, n]) -> u64 {
  let _ = xs
  comptime square(n)
}

pub fn main(t: Test) -> Result[(), ()] {
  let small: Inline[u8, 2] = [1, 2]
  let large: Inline[u8, 4] = [1, 2, 3, 4]
  test.check(t, scaled(small) == 4u64)
  test.check(t, scaled(large) == 16u64)
  test.check(t, scaled(small) == 4u64)
  Ok(())
}
```

Expected:

No stdout; exit 0 (all checks pass).
<!-- /tour -->

## Standard library: HTTP

`http.fetch` needs a `Net`, a config, a method, a URL, headers and an optional
body. Network authority is a finite set of exact address/port/operation rules;
a URL cannot widen it, including on redirects. This complete program uses a
scripted peer from `Test`, so it needs no live server. Run a Test entry with
`hirsel run` just like the other source fixtures: the simulator supplies Test.

<!-- tour: conformance/LANG/LANG-9-tour-08-http-run.hir -->
[Runnable source](../conformance/LANG/LANG-9-tour-08-http-run.hir)

```hirsel
pub fn main(t: Test) -> Result[(), http.HttpError] {
  let n = test.net(
    t,
    [
      NetPeer(
        address: "tour.test",
        port: 80,
        resolves_to: [V4([127u8, 0u8, 0u8, 1u8])],
        tls: Plain(),
        connections: [
          Open([Send(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"), Close()])
        ]
      )
    ],
    []
  )
  let response = http.fetch(
    n,
    http.default_config(),
    Get(),
    "http://tour.test/",
    [],
    None()
  )?
  test.check(t, response.status == 200u16)
  test.check(t, response.body == b"ok")
  Ok(())
}
```

Expected:

No stdout; exit 0 (all checks pass).
<!-- /tour -->

For a real server, see [hello-web](../examples/hello-web/main.hir),
[hello-web-concurrent](../examples/hello-web-concurrent/main.hir) and
[todo-api](../examples/todo-api/main.hir). They show listen/accept,
`http.receive_request` with head/body limits, `http.respond`, connection cleanup
and application routing. The concurrent examples currently run in the interpreter.

## File I/O: read, replace, handle missing files

`FileError` is the core type shared by file operations. Match `Error(NotFound())`
for an absent file; propagate other errors. Atomic replacement writes a temporary
file, then `file.commit` publishes it. Dropping it before commit aborts the write.

<!-- tour: conformance/LANG/LANG-9-tour-11-file-errors-run.hir -->
[Runnable source](../conformance/LANG/LANG-9-tour-11-file-errors-run.hir)

```hirsel
pub fn main(t: Test) -> Result[(), FileError] {
  let dir = test.dir(t, [(path.of("input.txt"), b"hello\n")])
  let data = file.read_bytes(dir, path.of("input.txt"))?
  let mut output = file.create_atomic(dir, path.of("output.txt"))?
  file.write_all(dir, mut output, data)?
  file.commit(dir, take output)?
  test.check(t, file.read_bytes(dir, path.of("output.txt")) == Ok(b"hello\n"))
  let missing = match file.read_bytes(dir, path.of("missing.txt")) {
    Ok(_) => false,
    Error(NotFound()) => true,
    Error(e) => return Error(e),
  }
  test.check(t, missing)
  Ok(())
}
```

Expected:

No stdout; exit 0 (all checks pass).
<!-- /tour -->

## Files and buffered I/O

File operations need both the `Dir` and its branded `File of dir`.
`file.open` reads; `file.create` creates/truncates an append-only output.
Keep one `bufio.Reader` per source while unread bytes remain. Reads may be short;
line helpers keep their newline. Bound accumulated input separately from buffer
capacity. Discard buffer state before seeking or reading the file directly.

The fake directory below confines effects to the test. `file.create_atomic`
starts a replacement, `file.write_all` supplies its bytes, and consuming
`file.commit` publishes it. Dropping an uncommitted atomic file aborts it,
including on an early `?`.

<!-- tour: conformance/LANG/LANG-9-tour-09-files-bufio-run.hir -->
[Runnable source](../conformance/LANG/LANG-9-tour-09-files-bufio-run.hir)

```hirsel
pub type AppError = Files(FileError) | Read(bufio.ReadError[FileError])

pub fn main(t: Test) -> Result[(), AppError] {
  let dir = test.dir(t, [(path.of("input.txt"), b"one\ntwo\n")])
  let mut f = file.open(dir, path.of("input.txt")) ? Files
  let mut reader = bufio.new(4)
  let first = bufio.file_read_line(dir, mut f, mut reader, 32) ? Read
  let second = bufio.file_read_line(dir, mut f, mut reader, 32) ? Read
  test.check(t, first == "one\n")
  test.check(t, second == "two\n")
  let mut output = file.create_atomic(dir, path.of("output.txt")) ? Files
  file.write_all(dir, mut output, string.as_bytes(first)) ? Files
  file.commit(dir, take output) ? Files
  test.check(t, file.read_bytes(dir, path.of("output.txt")) == Ok(b"one\n"))
  Ok(())
}
```

Expected:

No stdout; exit 0 (all checks pass).
<!-- /tour -->

## Capabilities, grants, stdin, arguments and environment

Roots arrive as entry parameters; each Io projection is fallible. `arg.count`
and `arg.text` use the explicit zero-based argument snapshot, excluding the
executable. `env.get` distinguishes a denied key (`Error(NotGranted())`) from a
granted unset key (`Ok(None())`). Io alone grants neither stdin nor stdout.
`input.read_line` returns `Some` for a final unterminated line and `None` at EOF.

Run the following in the default simulator (empty Args, Env and stdin), or use
`hirsel run conformance/CAP/CAP-1-tour-10-inputs-run.hir --grants docs/tour/inputs.json </dev/null`
for the same output with host streams. The checked-in template grants stdout,
stdin, empty Args and empty Env; it grants no files or network. To exercise data,
put strings in its `args` array and exact key/value snapshots in `env`, then pipe
input to the command. Environment snapshots never fall back to process variables.

<!-- tour: conformance/CAP/CAP-1-tour-10-inputs-run.hir -->
[Runnable source](../conformance/CAP/CAP-1-tour-10-inputs-run.hir)

```hirsel
pub type AppError = Grant(GrantError) | Arguments(ArgError) | Environment(EnvError) | Input(
  InputError
) | Output(OutputError)

pub fn main(rt: Io, args: Args, environment: Env) -> Result[(), AppError] {
  let name = if arg.count(args) == 0 {
    "tour"
  } else {
    arg.text(args, 0) ? Arguments
  }
  let label = match env.get(environment, "TOUR_LABEL") {
    Ok(Some(value)) => take value,
    Ok(None()) => "unset",
    Error(NotGranted()) => "unset",
    Error(e) => return Error(Environment(e)),
  }
  let output = io.stdout(rt) ? Grant
  out.print(output, name) ? Output
  out.print(output, "\n") ? Output
  out.print(output, label) ? Output
  out.print(output, "\n") ? Output
  let input = io.stdin(rt) ? Grant
  while true {
    match input.read_line(input) ? Input {
      Some(line) => out.print(output, line) ? Output,
      None() => return Ok(()),
    }
  }
  Ok(())
}
```

Expected:

```text
tour
unset
```
<!-- /tour -->

For host files, add a mount such as
`{"path":"data","host":"/absolute/host/directory","access":"read"}`
to `io.dirs`, acquire `io.dir(rt, path.of("data"))`, then use a relative filename
inside it. Use `read_write` when writes are intended. Paths cannot escape their
directory capability. [Building](building.md#build-and-launch-a-program) explains
grants templates and bound run records in detail.

## Tasks, channels and mutexes

`task.scope` joins all children before it exits. Await or detach every handle;
detach still keeps a child inside the scope. Child captures must be sendable:
plain values, moved unique data, copied `Shared` roots and scheduling handles.
A child cannot borrow a parent's counted list. Receivers may move into task
captures but cannot travel as channel payloads. A task closure may take an
immutable capture and rebind it mutable inside its own body.

A mutex body has `uses {}`: no captured authority, I/O, channel operations or
other lock acquisitions. Compute under the lock, then perform effects outside.
A failed mutex body kills the mutex. A channel send consumes its payload and
returns it with the error if delivery fails. This example returns one value
through a channel and explicitly joins the child; its output is independent of
which task gets scheduled first.

<!-- tour: conformance/CONC/CONC-2-tour-11-concurrency-run.hir -->
[Runnable source](../conformance/CONC/CONC-2-tour-11-concurrency-run.hir)

```hirsel
pub type AppError = Grant(GrantError) | Output(OutputError) | Channel(ChannelError)

pub fn main(rt: Io) -> Result[(), AppError] {
  let tasks = io.tasks(rt) ? Grant
  let received = task.scope(
    tasks,
    fn(s: Scope) -> Result[u64, ChannelError] {
      let total: Mutex[u64] = mutex.create(tasks, 0u64)
      let (tx, rx): (Sender[u64], Receiver[u64]) = channel.open(tasks)
      let child = task.async(
        s,
        fn() -> () {
          let value = mutex.with(
            tasks,
            total,
            fn(mut n: unique u64) -> u64 uses {} {
              n += 7
              n
            }
          )
          let mut sender = take tx
          let _ = channel.send(tasks, mut sender, value)
        }
      )
      let mut receiver = take rx
      let value = channel.receive(tasks, mut receiver)?
      task.await(s, take child)
      Ok(value)
    }
  ) ? Channel
  let output = io.stdout(rt) ? Grant
  out.print(output, u64.to_string(received)) ? Output
  out.print(output, "\n") ? Output
  Ok(())
}
```

Expected:

```text
7
```
<!-- /tour -->

To wait for several children, add their handles to a `TaskSet` and hand it to
`task.all`, which returns every result in the order they were added, or the
first `Error` once it has cancelled and joined the children still running.
`task.select` instead returns the index and result of the first child to
finish and cancels the rest. A task set must be consumed like a handle.

<!-- tour: conformance/CONC/CONC-2-tour-11-task-all-run.hir -->
[Runnable source](../conformance/CONC/CONC-2-tour-11-task-all-run.hir)

```hirsel
pub type AppError = Grant(GrantError) | Output(OutputError) | Range(RangeError)

pub type RangeError = Empty(u64, u64)

fn total(from: u64, to: u64) -> Result[u64, RangeError] {
  if from > to {
    return Error(Empty(from, to))
  }
  let mut sum: u64 = 0
  let mut i = from
  while i <= to {
    sum += i
    i += 1
  }
  Ok(sum)
}

pub fn main(rt: Io) -> Result[(), AppError] {
  let tasks = io.tasks(rt) ? Grant
  let output = io.stdout(rt) ? Grant
  let sums = task.scope(
    tasks,
    fn(s: Scope) -> Result[List[u64], RangeError] {
      let mut set: TaskSet[Result[u64, RangeError]] of s = task.set(s)
      task.add(
        s,
        mut set,
        task.async(
          s,
          fn() -> Result[u64, RangeError] {
            total(1, 10)
          }
        )
      )
      task.add(
        s,
        mut set,
        task.async(
          s,
          fn() -> Result[u64, RangeError] {
            total(11, 20)
          }
        )
      )
      task.all(s, take set)
    }
  ) ? Range
  for sum in sums {
    out.print(output, f"{sum}\n") ? Output
  }
  Ok(())
}
```

Expected:

```text
55
155
```
<!-- /tour -->

## Arenas

Ordinary code needs no arena annotation. When storage lifetime matters,
`arena.with` supplies a scoped `Arena`; `T in a` places fresh blocks there.
Such data cannot leave the arena, even after `copy` or `unique`. Copy it to the
heap explicitly to keep it. Arenas contain no resources. A unique arena builder
may update its root; a nested arena mutation detaches to the heap.

<!-- tour: conformance/MEM/MEM-4-tour-12-arenas-run.hir -->
[Runnable source](../conformance/MEM/MEM-4-tour-12-arenas-run.hir)

```hirsel
pub type AppError = Grant(GrantError) | Output(OutputError)

pub fn main(rt: Io) -> Result[(), AppError] {
  let text: String = arena.with(
    fn(a: Arena) -> String {
      let local_text: String in a = "kept"
      copy_to(heap, local_text)
    }
  )
  let output = io.stdout(rt) ? Grant
  out.print(output, text) ? Output
  out.print(output, "\n") ? Output
  Ok(())
}
```

Expected:

```text
kept
```
<!-- /tour -->

## Testing with `test.*`

Tests are ordinary functions marked `@test`, optionally taking an explicit `Test`.
Run this suite with `hirsel test conformance/LANG/LANG-9-tour-13-testing-run.hir.json`.
Each selected function runs independently, so a failed check does not skip later tests.
Core `test.check`, `test.dir` and `test.net` provide assertions and confined
fakes. `std@1/test` adds structural equality, approximate floats, error extraction
and property checks. `for_all` runs exactly 100 cases from the explicit generator
and reports the Test seed and case index on failure. A false check fails LANG-9.

<!-- tour: conformance/LANG/LANG-9-tour-13-testing-run.hir -->
[Runnable source](../conformance/LANG/LANG-9-tour-13-testing-run.hir)

```hirsel
fn generate(t: Test) -> u64 {
  7
}

fn law(n: u64) -> bool uses {} {
  n + 0 == n
}

@test
fn assertions(t: Test) {
  test.check(t, 2u64 + 2 == 4)
  let xs: List[u64] = [1, 2]
  test.equal(t, xs, [1, 2])
  test.approx(t, 0.1f64 + 0.2, 0.3, 0.000001, 0.0)
  test.for_all(t, generate, law)
  let rejected: Result[u64, String] = Error("bad input")
  let error = test.expect_error(t, take rejected)
  test.check(t, error == "bad input")
}
```

Expected:

All selected @test functions pass.
<!-- /tour -->

## CSV and byte codecs

`std@1/csv` parses bounded records; base64 decodes byte fields, SHA-256 hashes
bytes, and hex renders the digest. `test.expect_ok` transfers a successful
Result payload. The [CSV-to-JSON tool](../examples/csvjson/main.hir) combines
these helpers with flags, buffered input and escaped structured error logging.

<!-- tour: conformance/SEM/SEM-3-tour-14-wire-run.hir -->
[Runnable source](../conformance/SEM/SEM-3-tour-14-wire-run.hir)

```hirsel
pub fn trial(t: Test) -> Result[(), ()] {
  let rows = std@1/test.expect_ok(
    t,
    std@1/csv.parse("body\nYWJj\n", std@1/csv.default_config())
  )
  let data = std@1/test.expect_ok(
    t,
    std@1/base64.decode(rows[1][0], Standard(), Required())
  )
  test.check(t, std@1/base64.encode(data, Standard(), Required()) == "YWJj")
  let digest = std@1/sha256.digest(data)
  let mut digest_bytes: List[u8] = []
  for byte in digest {
    list.push(mut digest_bytes, byte)
  }
  test.check(
    t,
    std@1/hex.encode(core@1/bytes.from_list(take digest_bytes)) == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
  )
  Ok(())
}
```

Expected:

No stdout; exit 0 (all checks pass).
<!-- /tour -->

## Native builds

Build the complete values program above into a standalone executable:

```sh
hirsel build conformance/LANG/LANG-2-tour-01-values-run.hir -o /tmp/tour-values
/tmp/tour-values --hirsel-grants docs/tour/stdout.json
```

Expected stdout is `7` followed by a newline, exit 0, exactly as in the values
section. The checked-in stdout template grants only stdout. Use `--release`
for LLVM O3, `--debug` for O0 with source line tables, `--debug --backend
cranelift` for the fastest compile, or `--target aarch64-linux` for the
supported cross target. The executable still
requires explicit launch grants; building it grants no authority.

## Documenting your package

Attach `///` comments to public declarations, then select the package manifest:

```sh
hirsel doc app/manifest.json -o app/api.md
hirsel doc app/manifest.json --json
```

A library manifest needs no entry. The page retains public signatures and full
comments, with stable definition-path anchors and references to dependencies.
Opaque representations and `internal` modules stay hidden. Bodies are not checked
or evaluated, and documentation examples are not run. See
[documenting a package](building.md#document-a-package) for the JSON fields and
publication layout. The [core/std API page](api.md) is generated by the same
command on `stdlib/std@1/hirsel.json`.

## Keeping this page runnable

Every Hirsel block above is rendered from its linked conformance program,
including the expected stdout. Run `python3 docs/tour/render.py` after editing a
source. The tour integration tests reject stale, missing or extra blocks and run
every listed program through both ordinary and checked interpretation. The normal
gate also enforces canonical source formatting and the passing ratchet. These
are ordinary corpus inputs for the on-demand native campaign. An additional
full-gate tour test retains every native-capable snippet even when corpus
coverage minimization would omit it; native tasks remain unavailable.

Continue with the [dogfood tools](../examples/README.md) for complete programs
that read files, build text, parse JSON and implement a command loop.
