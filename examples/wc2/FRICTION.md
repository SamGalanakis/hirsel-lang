# First-contact friction

Minutes are approximate elapsed investigation time; overlapping waits are not a benchmark.
Copied diagnostics normalize checkout paths and strip trailing display whitespace.
The table preserves the original incidents; the recipes below describe current rules.

| Tried | Exact diagnostic | Fix | Minutes lost | Class |
| --- | --- | --- | ---: | --- |
| Used `distinct` and `result` as ordinary names. | [Exact output](repros/incident-1.txt) | Renamed them `sorted_unique` and `output_lines`; LANG-14. [Minimal program](repros/incident-1.hir). | 1 | language friction |
| Considered the LANG-14 replacement suggestion. | [Exact output](repros/incident-2.txt) | Historical help suggested reserved `result`; use `value`. The misleading suggestion is a diagnostic defect, not permission to use a reserved name. [Minimal program](repros/incident-2.hir). | 0.5 | bug |
| Inferred a list from its elements or the other `if` arm. | [Exact output](repros/incident-3.txt) | Added `List[cli.Flag]` / `List[String]`; LANG-2. [Minimal program](repros/incident-3.hir). | 0.5 | language friction |
| Sorted a list after a mutable helper call. | [Exact output](repros/incident-4.txt) | Declared `@preserves_unique(lines)` on the append-only helper; MODE-2/CLO-2. [Minimal program](repros/incident-4.hir). | 1 | language friction |
| Returned `out.print(..)?` from a Result function. | [Exact output](repros/incident-5.txt) | Print with `?`, then return `Ok(())`; LANG-7/LANG-8. [Minimal program](repros/incident-5.hir). | 0.5 | language friction |
| Read buffered lines inside a collection loop. | [Exact output](repros/incident-6.txt) | Fixed by [natural-loop origin expiration](https://github.com/SamGalanakis/hirsel-lang/commit/8d85aeac8cb3c32b8669cc0550906234c18bf0a0); [application file loop restored](https://github.com/SamGalanakis/hirsel-lang/commit/a19ff14b9727354246a86d29663c28c62c80739a). The unchanged [12-line repro](repros/invalid-origin.hir) and restored app pass H, LLVM and Cranelift in the [exact-source regression](../../crates/hirsel-cli/tests/examples.rs); independent V still rejects forged early expiry. Verified 2026-10-22. | 4 | bug (fixed) |
| Named a local `counts`, also the name of an @test. | [Exact output](repros/incident-7.txt) | Renamed test `count_words`; LANG-4 applies across the entire module. [Minimal program](repros/incident-7.hir). | 0.5 | language friction |
| Took a record field directly. | [Exact output](repros/incident-8.txt) | Destructured the whole Scan with `let Scan(..) = take scan`; OWN-8. [Minimal program](repros/incident-8.hir). | 0.5 | language friction |
| Pushed strings from a taken nested sequence into a unique accumulator. | [Exact output](repros/incident-9.txt) | Even `for line in take scan_lines` lends each element; `unique line` copies it independently and preserves the accumulator contract; MODE-2. [Minimal program](repros/incident-9.hir). | 0.5 | language friction |
| Expected no implicit text when main returns Error, following the tour. | [Exact output](repros/returned-error.txt): `error: ()`, exit 1. | PROG-3 correctly specifies the launcher report without a stderr grant. Fixed the stale tour sentence; [minimal program](repros/returned-error.hir). | 0.5 | doc gap |
| Registered a new example with dynamically named `repository_manifest` paths, as tasks does. | `called `Result::unwrap()` on an `Err` value: Os { code: 2, kind: NotFound, message: "No such file or directory" }` in the remote harness (five selected tests). | Single-source examples now use `Example::new` with explicit `include_str!` inputs; added the registration note to examples/README.md. | 0.5 | doc gap |

## Current executable recipes

Each block is a complete program for a normal std-dependent package (`hirsel run
hirsel.json`). The linked rejected sources and diagnostics above remain historical
evidence; changing a recipe does not change a language rule.

### Prefix inference and names

[Untyped sequence](repros/incident-3.hir) rejects LANG-2: a sequence literal needs
an expected sequence type. Its elements, a later use, or a later `if` arm cannot
supply that type backwards. This remains intentional. Give the binding its type:

```hirsel
pub fn main() -> Result[(), ()] {
  let sources: List[String] = ["-"]
  Ok(())
}
```

[Reserved `result`](repros/incident-1.hir) rejects LANG-14; `distinct` is reserved
too. [A local named like a top-level function](repros/incident-7.hir) rejects
LANG-4 even if that function appears later in the module. Rename the binding or
function, including test functions. A complete repair for the latter is:

```hirsel
fn counts() {}

pub fn main() -> Result[(), ()] {
  let value = 0u64
  Ok(())
}
```

### Mutable helper promises

[The unannotated helper](repros/incident-4.hir) loses the caller's uniqueness
proof at `need(take xs)` (MODE-2). Native loans do not infer a callee's handback
contract from its body. Both `remember` and `collect` retain
`@preserves_unique(lines)`: each promises to return unique storage when it
receives it. The compiler checks that promise (CLO-2); it is not an unchecked
optimization. A minimal repair is:

```hirsel
@preserves_unique(xs)
fn add(mut xs: List[u64]) {
  list.push(mut xs, 1)
}

fn need(take xs: unique List[u64]) {}

pub fn main() -> Result[(), ()] {
  let mut xs: unique List[u64] = []
  add(mut xs)
  need(take xs)
  Ok(())
}
```

### Whole-value handback and row copies

[Moving a field](repros/incident-8.hir) rejects OWN-8 because it would leave the
record partly moved. Destructure its owner instead. `wc2` uses the same pattern
for `Scan` to recover its counts and unique line list:

```hirsel
type Row { values: List[u64] }

pub fn main() -> Result[(), ()] {
  let r = Row(values: [1])
  let Row(values: values) = take r
  Ok(())
}
```

[Appending `copy line`](repros/incident-9.hir) loses the unique accumulator proof
(MODE-2). Taking the sequence does not take each counted row: iteration lends
rows. `unique line` makes an independent deep copy; `copy line` may share storage.
Retain that explicit copy when the destination must remain unique:

```hirsel
fn need(take xs: unique List[String]) {}

pub fn main() -> Result[(), ()] {
  let source: List[String] = ["a"]
  let mut out: unique List[String] = []
  for line in source {
    list.push(mut out, unique line)
  }
  need(take out)
  Ok(())
}
```

This also explains `unique parts[0]` in `remember`, `unique parsed.positional`,
and `previous = Some(unique line)`: each copies borrowed counted data into an
independent owner. Explicit unique list bindings, `take lines`, the unique
`Scan.lines` field and `Result[unique Scan, AppError]` declare and transfer the
handback needed by sorting; they are retained. `read_stdin` remains a useful
helper, not a workaround for InvalidOrigin. The file loop is already inline;
[its original nested-loop repro](repros/invalid-origin.hir) is an accepted bug
regression, not an intentional rejection.

### Result completion

[Ending with `unit()?`](repros/incident-5.hir) rejects LANG-7: `?` extracts `()`;
it does not wrap success in the enclosing Result. Print/read with `?`, then
return `Ok(())`. Errors still propagate and clean up owned resources:

```hirsel
fn unit() -> Result[(), ()] { Ok(()) }

pub fn main() -> Result[(), ()] {
  unit()?
  Ok(())
}
```

The same rule explains `Ok(Scan(...))` in `read_stdin`. Returned entry errors
still cause the launcher's PROG-3 report; explicit stderr printing is separate.
