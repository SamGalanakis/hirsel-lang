# Borrow cursor recipes

A borrowed cursor is a read-only view whose **whole local name** can be rebound
within a fixed origin cover. An owned snapshot has its own counted shares and
ordinary mutable value semantics. Choose by ownership, not by whether a loop
looks like traversal. The authority is [LANG-15](spec/language.md#lang-15-bindings)
and [snapshot loans](spec/ownership.md#own-3-exclusive-access); the
[ownership repairs](ownership-repairs.md) explain copying, transfer and uniqueness.
This guide changes no rule and assumes no extra relation inference.

Every `hir` block below is checked as a separate std-free package by the CLI
recipe tests. The `paired-cursor` blocks use the focused paired recipe harness. The first comment states acceptance or the current rejection rule.
Accepted functions also run in the [example](../examples/borrow-cursor-recipes/main.hir).
Rejects deliberately have no executable entry.

## Direct enum descent: borrowed view and owned snapshot

This direct physical pattern is the shipped
[tree witness](../conformance/LANG/LANG-15-borrow-cursor-tree-ok.hir).
The borrowed initializer establishes `root` as the fixed cover; each child lies
inside it. The plain `u64` result needs no counted copy.

```hir
// expect: accept
type Tree = Leaf(n: u64) | Branch(left: box Tree, right: box Tree)

fn borrowed_tree(root: box Tree) -> u64 {
  let mut node = root
  while true {
    match node {
      Leaf(n: n) => {
        return n;
      },
      Branch(left: child, right: _) => {
        node = child
      },
    }
  }
  0
}

```

The owned version deliberately copies at initialization and at every descent.
Copying shares counted storage; later writes can detach it. It is still useful
when independent snapshots or mutation are required. It is not a blanket
candidate for deleting `copy`.

```hir
// expect: accept
type Tree = Leaf(n: u64) | Branch(left: box Tree, right: box Tree)

fn owned_tree(root: box Tree) -> u64 {
  let mut node = copy root
  while true {
    match node {
      Leaf(n: n) => {
        return n;
      },
      Branch(left: child, right: _) => {
        node = copy child
      },
    }
  }
  0
}

```

The smallest owned-refill counterexample fails LANG-15. A cursor replacement
must be a view; `copy`, `take`, construction and call results are owned.

```hir
// expect: reject LANG-15
fn refill(a: List[u64]) -> u64 {
  let mut node = a
  node = copy a
  list.len(node)
}
```

## Array descent and an explicit owned result

The array arm adapts the shipped
[JSON pointer witness](../conformance/LANG/LANG-15-borrow-cursor-json-pointer-ok.hir).
The length check precedes indexing. Rebinding borrows an element; `copy node` at
the result boundary creates the owned return value. The `in root` relation is
still required: copy retains the input's bounds. It is not a deep independent
`unique` repair or heap promotion.

```hir
// expect: accept
type Value = Text(String) | Array(List[Value])

fn array_result(root: Value) -> Value in root {
  let mut node = root
  while true {
    match node {
      Text(_) => {
        return copy node;
      },
      Array(items) => {
        if list.len(items) == 0 {
          return copy node;
        }
        node = items[0]
      },
    }
  }
  copy node
}

```

An escaping borrowed parameter cannot silently become a global owned result:

```hir
// expect: reject MODE-1
fn escape(a: List[u64]) -> List[u64] {
  let mut node = a
  node = a
  copy node
}
```

`core@1/map.get(members, key)` returns an **owned `Option[Value]`**. Matching it
does not make its payload a borrowed member place. The current reject is MODE-1
at `target = take next` below (not an invented getter-specific LANG-15 diagnostic).
Keep the current object recursion and hash lookup repair; a linear member scan
is a conformance witness, not a recommendation to change lookup complexity.
The full composition is preserved in
[the owned-getter reject](../conformance/MODE/MODE-1-borrow-cursor-owned-getter-bad.hir).

```hir
// expect: reject MODE-1
type Value = Text(String) | Object(Map[String, Value])

fn child(value: Value, key: String) -> Option[Value] in value uses {} {
  match value {
    Object(members) => core@1/map.get(members, key),
    _ => None(),
  }
}

fn walk(value: Value, key: String) -> Option[Value] in value uses {} {
  let mut target = copy value
  let Some(next) = child(target, key) else { return None() }
  target = take next
  Some(take target)
}
```

## Compare two Array roots with separate physical matches

Each cursor starts from its own borrowed root. Match `left` and `right`
separately so `xs` and `ys` remain physical views of their respective roots.
Check lengths before indexing and return immediately for two empty arrays.
Compare every non-tail sibling recursively before rebinding both cursors to
that array's last child; a matching tail cannot hide an unequal earlier child.
The `Tagged` arm compares boxed values structurally and returns.

```hir paired-cursor
// expect: accept
type PairValue = Text(String) | Leaf | Array(List[PairValue]) | Tagged(value: box PairValue)

fn same(a: PairValue, b: PairValue) -> bool uses {} {
  let mut left = a
  let mut right = b
  while true {
    match left {
      Array(xs) => match right {
        Array(ys) => {
          let n = core@1/list.len(xs)
          if n != core@1/list.len(ys) {
            return false
          }
          if n == 0 {
            return true
          }
          let mut i = 0u64
          while i + 1 < n {
            if !same(xs[i], ys[i]) {
              return false
            }
            i += 1
          }
          left = xs[n - 1]
          right = ys[n - 1]
        },
        _ => return false,
      },
      Text(s) => return match right {
        Text(t) => s == t,
        _ => false,
      },
      Leaf() => return match right {
        Leaf() => true,
        _ => false,
      },
      Tagged(value: x) => return match right {
        Tagged(value: y) => x == y,
        _ => false,
      },
    }
  }
  false
}

```

A computed tuple `(a, b)` constructs an owned value. Destructuring it does not
recover the original roots' physical view origins, even with `in {a, b}`:

```hir paired-cursor
// expect: reject MODE-1
type Value = Leaf | Array(List[Value]) | Tagged(value: box Value)
fn bad(a: Value, b: Value) -> bool uses {} {
  let mut c: Value in {a, b} = a
  match (a, b) {
    (Array(xs), _) => { c = xs[0u64] },
    _ => {},
  }
  c == a
}
```

## Keep boxed-chain cursors boxed

`Value` and `box Value` are different cursor types. Structural matching can
inspect a box, but `Tagged(value: inner)` binds the existing **boxed** child.
This cursor therefore stays `box Value` for the entire chain:

```hir paired-cursor
// expect: accept
type PairValue = Text(String) | Leaf | Array(List[PairValue]) | Tagged(value: box PairValue)

fn tag_depth(value: box PairValue) -> u64 uses {} {
  let mut cursor = value
  let mut n = 0u64
  while true {
    match cursor {
      Tagged(value: inner) => {
        cursor = inner
        n += 1
      },
      _ => return n,
    }
  }
  n
}

```

Assigning that child directly to a `Value` cursor fails LANG-7:

```hir paired-cursor
// expect: reject LANG-7
type Value = Leaf | Array(List[Value]) | Tagged(value: box Value)
fn bad(value: Value) -> bool uses {} {
  let mut c = value
  match c {
    Tagged(value: inner) => { c = inner },
    _ => {},
  }
  true
}
```

A helper returning `Value in value` still returns an owned value. The relation
constrains its bounds; it does not return a borrow or make a call result a view.
Reconstructing the payload with `unboxed` therefore fails LANG-15 as a refill:

```hir paired-cursor
// expect: reject LANG-15
type Value = Leaf | Array(List[Value]) | Tagged(value: box Value)
fn unboxed(value: box Value) -> Value in value uses {} {
  match value {
    Leaf() => Leaf(),
    Array(items) => Array(copy items),
    Tagged(value: inner) => Tagged(value: copy inner),
  }
}
fn bad(value: Value) -> bool uses {} {
  let mut c = value
  match c {
    Tagged(value: inner) => { c = unboxed(inner) },
    _ => {},
  }
  true
}
```

No single cursor here crosses both Array elements (`Value`) and boxed children
(`box Value`) without reconstruction. Keep the original recursive
[YAML equality fallback](../stdlib/std@1/yaml.hir): its owned tuple, hash lookup
and recursive `unboxed` results retain their existing ownership and failure order.
These recipes do not migrate that implementation or establish an RC or timing win.

## Paired roots preserve the old alias

After rebinding `c`, `old` still reads the first root. This returns 10 for
`[3]` and `[7]`; it does not retarget `old` to the second root:

```hir paired-cursor
// expect: accept
fn read(a: List[u64], b: List[u64]) -> u64 uses {} {
  let mut c: List[u64] in {a, b} = a
  let old = c
  c = b
  old[0u64] + c[0u64]
}

```

The old alias keeps its loan live; rebinding the cursor does not allow a write
to the first root before that alias's last use:

```hir paired-cursor
// expect: reject MODE-1
fn bad(mut a: List[u64], b: List[u64]) -> u64 uses {} {
  let mut c: List[u64] in {a, b} = a
  let old = c
  c = b
  a = [9u64]
  old[0u64] + c[0u64]
}
```

A borrowed cursor also cannot escape as an unbounded owned result:

```hir paired-cursor
// expect: reject MODE-1
fn bad(a: List[u64]) -> List[u64] uses {} {
  let mut c = a
  c = a
  c
}
```

## Explicit enclosing cover: siblings, reset and the old view

The pattern is outside cursor assignment. `in p` explicitly covers both fields,
so visiting `p.b` after `p.a`, then resetting to `p.a`, is allowed. The declaration
fixes the cover; it never grows with assignments. `saved` keeps the first view,
independently of `node`'s later values. The function returns 4 for lengths 1 and 2.

```hir
// expect: accept
type Pair { a: List[u64], b: List[u64] }

fn sibling_reset(p: Pair) -> u64 {
  let Pair(a: first, b: _) = p
  let mut node: List[u64] in p = first
  let saved = node
  node = p.b
  let both = list.len(saved) + list.len(node)
  node = p.a
  both + list.len(node)
}

```

A narrow initializer does not admit an unrelated root:

```hir
// expect: reject MODE-1
fn unrelated(a: List[u64], b: List[u64]) -> u64 {
  let mut node = a
  node = b
  list.len(node)
}
```

Naming `in p.a` instead of `in p` also excludes `p.b`; the shipped
[field-cover twins](../conformance/MODE/MODE-1-borrow-cursor-field-ok.hir)
cover that distinction. Tree reset is independently covered by the shipped
[reset witness](../conformance/LANG/LANG-15-borrow-cursor-reset-ok.hir).
A whole-name rebind writes only the view holder. It grants no projected write,
mutable argument, compound assignment or take permission:

```hir
// expect: reject LANG-15
fn write(a: List[u64]) -> u64 {
  let mut node = a
  node[0] = 9
  list.len(node)
}
```
```hir
// expect: reject OWN-1
fn consume(take a: List[u64]) -> u64 { list.len(a) }

fn move_view(a: List[u64]) -> u64 {
  let mut node = a
  consume(take node)
}
```

## Saved snapshots keep loans until their last use

Here `saved` is a borrowed snapshot of the old view, not an owned copy. Rebinding
`node` to `b` releases only `node`'s holding of `a`. The read of `saved` must finish
before mutating `a`. A closure borrowing the binding storage itself is different:
it can conflict with rebinding that storage (see the reject below).

```hir
// expect: accept
fn saved_snapshot(mut a: List[u64], b: List[u64]) -> u64 {
  let mut node: List[u64] in {a, b} = a
  let saved = node
  node = b
  let answer = list.len(saved) + list.len(node)
  // saved's last use ended its loan on a; rebinding node alone did not.
  list.push(mut a, 9)
  answer
}

```
```hir
// expect: reject OWN-3
fn live_snapshot(mut a: List[u64], b: List[u64]) -> u64 {
  let mut node: List[u64] in {a, b} = a
  let saved = node
  node = b
  list.push(mut a, 9)
  list.len(saved) + list.len(node)
}
```
```hir
// expect: reject MODE-1
fn captured(a: List[u64]) -> u64 {
  let mut node = a
  let watch = fn() -> u64 { list.len(node) }
  node = a
  watch()
}
```

## Ordinary mutable values are different

`let mut count = 0u64; count += 1` is plain scalar state, not a borrow cursor.
The package checks this alongside the walks. A mutable transaction explicitly
copies its input and can mutate the copied value. Keep that copy:

```hir
// expect: accept
fn transaction(a: List[u64]) -> List[u64] in a {
  let mut work = copy a
  list.push(mut work, 9)
  take work
}

```

The persistent [ordered map](../stdlib/std@1/ordered_map.hir) has only
`K: copyable, V: copyable` and carries a comparator closure. `copyable` does not
imply `data`. Its generic trees cannot silently acquire `data` bounds or become
borrow cursors. The focused
[generic twins](../conformance/LANG/LANG-15-borrow-cursor-generic-ok.hir)
keep the owned copy form and reject this borrowed rebind:

```hir
// expect: reject LANG-15
type Tree[T: copyable] = Leaf(T) | Children(List[Tree[T]])

fn generic[T: copyable](root: Tree[T]) -> u64 {
  let mut node = root
  match node {
    Leaf(_) => 1,
    Children(items) => {
      if list.len(items) > 0 { node = items[0] }
      0
    },
  }
}
```

Closures fail the `data` condition; resources fail the `copyable` condition.
Their read-only borrowed locals do not gain cursor rebinding. The first
rejection is LANG-15 in each case. The resource assignment additionally reports
OWN-1 for trying to move the borrowed `b`; the guide test preserves the complete
JSON-lines stream and checks the first primary span:

```hir
// expect: reject LANG-15
fn closures(a: fn() -> u64, b: fn() -> u64) -> u64 {
  let mut node = a
  node = b
  node()
}
```
```hir
// expect: reject LANG-15
resource type R { n: u64 }

fn resources(a: R, b: R) -> u64 {
  let mut node = a
  node = b
  node.n
}
```

The projected/field-match producer repair was tracked separately as
`borrow-cursor-field-match-13` and landed in
[`955beb25`](https://github.com/SamGalanakis/hirsel-lang/commit/955beb25c36972b4920007539e6ca8f702f8c1cf).
This guide keeps the earlier shipped direct enum patterns and enclosing-scope
record patterns; it adds no field-match witness or compiler change.

## Execute the guide

From the repository root, with a matching toolchain:

```sh
hirsel check examples/borrow-cursor-recipes/hirsel.json
hirsel test examples/borrow-cursor-recipes/hirsel.json
hirsel test --native --backend llvm examples/borrow-cursor-recipes/hirsel.json
hirsel test --native --backend cranelift examples/borrow-cursor-recipes/hirsel.json
python3 tools/buck2/prepare.py
.buck2/bin/buck2 targets 'root//:'
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=borrow_cursor_recipes_13
```

The last selection runs three Rust tests: every guide block through public
`check --json` (rule and primary byte span for each reject), selected shipped and
new corpus twins through the same checker, and the package's test plus executable
entry on H, LLVM and Cranelift. It builds each standalone entry once per native
backend and reuses that path, following [native verification](native-verification.md).
The paired extension is checked by:

```sh
.buck2/bin/buck2 test //:hirsel-cli-test-cli-shard-1 -- --test-arg=paired_cursor_recipes_17
.buck2/bin/buck2 test //:hirsel-validate-test-it -- --test-arg=borrow_cursor_contracts_8
.buck2/bin/buck2 test -c hirsel.conformance_paths=conformance/LANG/LANG-15-paired-cursor-recipes-17- //:gate-conformance-selected-shard-1
```

It checks the eight labelled blocks (three accepted, five rejected) and their
corpus twins, and runs the extended package on the interpreter, checked heap,
LLVM and Cranelift. Each native entry is built once per backend and reused.
The bounded vectors cover independent roots, empty and unequal arrays,
unequal non-tail siblings, nested tails, boxes and saved aliases; they are not
a proof for all inputs. Zero selected tests is a failure.

Cursor rebinding itself performs no count operation. Final owned result sharing
is a separate operation and is not included in that statement. The existing
`rc_borrow_cursor_8` source control remains selected. The recipe harness also
uses the existing native RC instrumentation for a bounded scan, attributing
transport and final owned result sharing to separate functions;
no timing ratio or general allocation-free claim follows. Tree construction,
owned snapshots, copies at result boundaries and later mutations can cost work.

Sources: local Hirsel conformance and normative rules pinned in the task audit
at `8cd5f338c97f9afc2cadcf08fd49b724e91d3d21`, refreshed against current main.
Adaptations: reduce JSON to Text/Array; keep direct enum payload patterns; add
an empty-array guard, enclosing-root sibling/reset, a saved view's last-use
control, and explicit result copies with Hirsel relations. No external algorithm
or source is ported, and no external license/notice is introduced.

Paired recipe provenance: the already checked Hirsel `cursor-pair-design-16`
separate-physical and direct-box-projection examples. The Array function's only
semantic adaptations are the `PairValue` type name and a `Text` arm matching the
existing example. The boxed walk and saved-alias function retain their original
structure. Added separately
constructed roots and boundary vectors; canonical formatting follows this repo.
No external implementation or license notice applies. No language, compiler,
standard-library, independent validator or public README rule changes.
