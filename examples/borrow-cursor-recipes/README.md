# Borrow cursor recipes

Read the [guide](../../docs/borrow-cursor-recipes.md) for accepted and rejected
forms, fixed origin covers, snapshot loans and explicit owned result copies.
`main.hir` compares borrowed and owned tree descent, nested array selection,
empty arrays, sibling/reset covers, saved views, mutable transaction copies and
plain counters. It also compares two independent Array roots using separate
physical matches, walks boxed tag chains and preserves an alias across rebinding.
Unequal non-tail siblings are checked before tail descent. The guide includes
computed-pair, owned-refill, box-type, stale-root and escape refusal twins.
Success is silent with exit 0; the one `@test` checks the same observations. No capabilities or std dependency are needed.

```sh
hirsel test examples/borrow-cursor-recipes/hirsel.json
hirsel run examples/borrow-cursor-recipes/hirsel.json
```

For native launch, build once per backend and run the exact paths as described
in [native verification](../../docs/native-verification.md). The guide's focused
CLI target checks all snippets and H/LLVM/Cranelift package controls.

Adapted from shipped Hirsel LANG-15 tree/JSON, MODE-1 field-cover and OWN-3
snapshot controls; the guide lists the changes and rule boundaries. The paired
Array and box-chain extension adapts the checked Hirsel `cursor-pair-design-16`
examples, with a separate type name, a Text arm and additional independent input
vectors. No external source port or new license applies.
