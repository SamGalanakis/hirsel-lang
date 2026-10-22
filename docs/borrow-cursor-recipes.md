# Borrow cursor recipes

A borrowed cursor is a read-only view whose **whole local name** can be rebound
within a fixed origin cover. An owned snapshot has its own counted shares and
ordinary mutable value semantics. Choose by ownership, not by whether a loop
looks like traversal. The authority is [LANG-15](spec/language.md#lang-15-bindings)
and [snapshot loans](spec/ownership.md#own-3-exclusive-access); the
[ownership repairs](ownership-repairs.md) explain copying, transfer and uniqueness.
This guide changes no rule and assumes no extra relation inference.

Every `hir` block below is checked as a separate std-free package by the CLI
recipe test. The first comment states acceptance or the current rejection rule.
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
Zero selected tests is a failure.

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
