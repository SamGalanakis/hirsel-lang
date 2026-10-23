# JSON, text and time recipes

Start with the task, then look up its public module in the selected
[std manifest](../stdlib/std@1/hirsel.json). A source's `module` field assigns its
namespace; its physical filename does not. For example, `time_format.hir`,
`time_civil.hir` and `time.hir` all contribute to `time`. No imports are needed.
The [full signature index](api.md), [std catalogue](spec/std-library.md) and
[core catalogue](spec/core-library.md) are the complete references.

| Task / search spelling that failed | Public spelling | Contract at the call |
| --- | --- | --- |
| Object lookup: `json.member` | `json_access.member(value: json.Value, key: String) -> Option[json.Value] in value uses {}` | Borrows both arguments; result is related only to `value`. Missing key or wrong kind gives `None()`, present null gives `Some(Null())`. |
| Array lookup: `json.element`, `json.array_len` | `json_access.element(value: json.Value, index: u64) -> Option[json.Value] in value uses {}` | Borrows value; absent index or wrong kind gives `None()`. For length use `json_access.as_array(value: json.Value) -> Option[List[json.Value]] in value uses {}`, then core `list.len`. |
| JSON string: `json.String`, `json.Text` | Contextual `Text(String)` in `json.Value` | Variants are bare names selected by the expected type. `json.stringify(value: json.Value) -> Result[unique String, json.JsonError] uses {}` borrows the value and returns independent encoded text. |
| Checked owned UTF-8: `string.from_utf8`, `text.from_bytes` | Core `string.from_bytes(take bytes: Bytes) -> Result[String, Bytes]` | Consumes the owned bytes. `Ok` contains valid text; `Error` returns exactly the original invalid bytes. `@preserves_unique(bytes)` preserves supplied uniqueness on either exit. |
| Format supplied civil time: `time_format.format`, `time_format.FormatError` | `time.format(value: time.BrokenDownTime, pattern: String) -> Result[unique String, time.FormatError] uses {}` | Borrows value and pattern, returns independent text. Error is `Invalid(u64) \| NoOffset \| OutOfRange`; `Invalid` gives the pattern byte offset. |

The implementations are [JSON access](../stdlib/std@1/json_access.hir),
[JSON encoding](../stdlib/std@1/json.hir) and
[time formatting](../stdlib/std@1/time_format.hir). JSON's error type is
`Syntax(u64) | DuplicateKey(u64) | Nesting(u64) | InvalidNumber`.
`json.write_text_into(text: String, mut builder: List[u8]) -> () uses {}`
appends one quoted JSON string when writing a larger document.

Core supplies `string.from_bytes`, `string.as_bytes(s: String) -> Bytes in s`,
`string.byte_len` and position/slice operations. Ordinary std source extends the
same `string` head with `find`, `trim`, `concat`, `split` and others; see
[string.hir](../stdlib/std@1/string.hir). Core and std heads merge under
[LANG-4](spec/language.md#lang-4-names); full spellings such as
`core@1/string.from_bytes` and `std@1/string.find` disambiguate definitions.
A lookup failure is not permission to substitute a similarly spelled operation:
`json.array_next` advances a streaming reader, not a DOM array lookup. Search
public declarations and the manifest; private helpers are not application APIs.

## Complete package setup

Create a directory `recipe/` directly under the repository root. Save this as
`recipe/hirsel.json`, and save **one** of the complete programs below as
`recipe/main.hir`. The dependency path is relative to this manifest; adjust it
if placing the package elsewhere. Every entry reaches its value checks, returns
`Ok(())` on success and needs no host capability or clock.

<!-- recipe-manifest -->
```json
{
  "schema": 1,
  "package": {"name": "std_recipe", "version": "0"},
  "entry": {"module": [], "name": "main"},
  "sources": [{"path": "main.hir", "module": []}],
  "dependencies": [{
    "manifest": "../stdlib/std@1/hirsel.json",
    "package": {"name": "std", "version": "1"},
    "expose": []
  }]
}
```

```sh
hirsel check --json recipe/hirsel.json
hirsel export --tir recipe/hirsel.json -o /tmp/std-recipe.tir
hirsel run recipe/hirsel.json
```

Source check, reachable export and execution are separate checks. The
[recipe tests](../crates/hirsel-driver/tests/it-stdlib/std_api_recipes_24.rs)
compile these exact Markdown programs with the manifest above and execute each
under the interpreter and checked heap. `failure.check` makes the handwritten
expected values executable; applications can match the same Options/Results to
report their own errors.

## Object member lookup

The selected value borrows `document`. Keep that document alive while using the
selection. `unique selected` would explicitly detach a copy if it must outlive
the document; a shallow `copy` does not erase the relation.

<!-- recipe:object -->
```hirsel
pub fn main() -> Result[(), ()] uses {} {
  let mut fields: Map[String, json.Value] = map.empty()
  map.insert(mut fields, "name", Text("Ada"))
  map.insert(mut fields, "nil", Null())
  let document: json.Value = Object(take fields)
  failure.check(json_access.member(document, "name") == Some(Text("Ada")), TextInvariant())
  failure.check(json_access.member(document, "nil") == Some(Null()), TextInvariant())
  failure.check(json_access.member(document, "missing") == None(), TextInvariant())
  let empty: json.Value = Object(map.empty())
  failure.check(json_access.member(empty, "name") == None(), TextInvariant())
  let wrong: json.Value = Array([])
  failure.check(json_access.member(wrong, "name") == None(), TextInvariant())
  Ok(())
}
```

## Array element and length

Indices are zero based `u64`. Both the element and the list observation retain
the array's lifetime. Wrong-kind and missing-index checks are distinct.

<!-- recipe:array -->
```hirsel
pub fn main() -> Result[(), ()] uses {} {
  let document: json.Value = Array([Text("first"), Null()])
  failure.check(json_access.element(document, 0) == Some(Text("first")), TextInvariant())
  failure.check(json_access.element(document, 1) == Some(Null()), TextInvariant())
  failure.check(json_access.element(document, 2) == None(), TextInvariant())
  let empty: json.Value = Array([])
  failure.check(json_access.element(empty, 0) == None(), TextInvariant())
  let wrong: json.Value = Text("not an array")
  failure.check(json_access.element(wrong, 0) == None(), TextInvariant())
  let Some(items) = json_access.as_array(document) else { return Error(()) }
  failure.check(list.len(items) == 2, TextInvariant())
  Ok(())
}
```

## JSON text escaping

The `json.Value` annotation supplies the expected variant type. Passing bare
`Text(...)` directly to `json.stringify` also gets that context from its
parameter. Encoding handles quotes, backslashes, control characters and Unicode;
interpolating raw user text into JSON does not do that encoding.

<!-- recipe:json-text -->
```hirsel
pub fn main() -> Result[(), ()] uses {} {
  let value: json.Value = Text("quote: \" slash: \\ line:\n雪 💖")
  let Ok(encoded) = json.stringify(value) else { return Error(()) }
  failure.check(encoded == "\"quote: \\\" slash: \\\\ line:\\n雪 💖\"", TextInvariant())
  let mut builder: List[u8] = []
  json.write_text_into("\"\n雪", mut builder)
  failure.check(bytes.from_list(take builder) == b"\"\\\"\\n\xe9\x9b\xaa\"", TextInvariant())
  Ok(())
}
```

## Owned valid UTF-8

`bytes.from_list(take builder) -> Bytes` consumes a mutable byte builder into
owned read-only bytes. `take data` then transfers those bytes to the checked
constructor. Borrowed `string.as_bytes` alone does not supply an owned move;
use an explicit independent copy when the source must remain live.

<!-- recipe:utf8-valid -->
```hirsel
pub fn main() -> Result[(), ()] uses {} {
  let mut builder: List[u8] = [104, 101, 108, 108, 111]
  let data = bytes.from_list(take builder)
  let Ok(text) = string.from_bytes(take data) else { return Error(()) }
  failure.check(text == "hello", TextInvariant())
  let multilingual = unique string.as_bytes("ศไทย中华Việt Nam")
  let Ok(decoded) = string.from_bytes(take multilingual) else { return Error(()) }
  failure.check(decoded == "ศไทย中华Việt Nam", TextInvariant())
  Ok(())
}
```

## Owned invalid UTF-8 preserves bytes

The error payload is `Bytes`, not a replacement string or a decoder-error
record. Inspect or retain it by matching `Error(original)`.

<!-- recipe:utf8-invalid -->
```hirsel
pub fn main() -> Result[(), ()] uses {} {
  let mut builder: List[u8] = [104, 101, 108, 108, 111, 255]
  let data = bytes.from_list(take builder)
  match string.from_bytes(take data) {
    Ok(_) => return Error(()),
    Error(original) => failure.check(original == b"hello\xff", TextInvariant()),
  }
  Ok(())
}
```

## Format an explicitly supplied civil value

`time.BrokenDownTime` contains `datetime: time.DateTime` and
`offset: Option[i32]`, seconds east of UTC. These are supplied data; formatting
reads no clock. `Date` and `TimeOfDay` are opaque. Use their public checked constructors:
`date.new(y: i32, m: u8, d: u8) -> Option[time.Date] uses {}` and
`time_of_day.new(h: u8, m: u8, s: u8, ns: u32) -> Option[time.TimeOfDay] uses {}`.
Both return `None()` for invalid fields. `DateTime` and `BrokenDownTime` are
public records. For text input, see the parsers in the
[std catalogue](spec/std-library.md).
For a timestamp use
`time.broken_down(value: time.Timestamp, offset: i32) -> Result[time.BrokenDownTime, time.TimeError] uses {}`;
its error is `OutOfRange`. This function, too, is registered under `time`.

<!-- recipe:time -->
```hirsel
pub fn main() -> Result[(), ()] uses {} {
  let Some(day) = date.new(2024, 7, 14) else { return Error(()) }
  let Some(wall) = time_of_day.new(17, 31, 59, 123456789) else { return Error(()) }
  let civil = time.DateTime(date: day, time: wall)
  let value = time.BrokenDownTime(datetime: civil, offset: Some(-14400))
  let Ok(rendered) = time.format(value, "%a, %d %b %Y %H:%M:%S.%f %z (%j)") else {
    return Error(())
  }
  failure.check(rendered == "Sun, 14 Jul 2024 17:31:59.123456789 -0400 (196)", TextInvariant())
  failure.check(time.format(value, "%q") == Error(Invalid(0)), TextInvariant())
  let no_offset = time.BrokenDownTime(datetime: civil, offset: None())
  failure.check(time.format(no_offset, "%z") == Error(NoOffset()), TextInvariant())
  Ok(())
}
```

The safe lookup shapes follow serde_json `src/value/index.rs:53–89` at
`20572a6d066958542d79cdd6cf1a7cd9f0c8e1f8`. The UTF-8 usage/value vectors follow
Rust `library/alloc/src/string.rs:510–559` and
`library/alloctests/tests/string.rs:45–60` at
`f6c1107f8be13e48465e12c0fd26e6a39fe18e4f`. Hirsel uses explicit moves,
`Option`/`Result` contextual variants and a raw byte error payload. No validator
or accessor implementation is ported here. Time's fixed output and failure
vectors come from the shipped
[formatter controls](../conformance/SEM/SEM-1-time-strftime-run.hir).
