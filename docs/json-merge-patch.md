# JSON configuration overlays

`std@1/json_merge_patch.apply(take target, patch, max_depth)` returns
`Result[json.Value, MergeError]` and implements RFC7396. An object patch promotes
any scalar/array target to an empty object, deletes members whose patch value is
null, and merges the other members. A nonobject patch replaces the whole target.
Arrays replace wholesale; RFC6902 operation arrays are ordinary replacement
values here. This module does not interpret JSON pointers.

An object member cannot be assigned JSON null using a merge patch: null means
delete. A root null patch does replace the complete target with null. Empty object
patches preserve existing objects but promote nonobjects to empty objects.
Numbers retain their exact `Number(String)` spelling, including `1`, `1.0`, and
`1e0`. Keys, including `__proto__`, `constructor`, `prototype`, and Unicode text,
are ordinary JSON map keys. There are no JavaScript prototype rules.

```hirsel
let Ok(base) = std@1/json.parse("{\"service\":{\"port\":8080,\"debug\":true}}") else { return Error(()) }
let Ok(patch) = std@1/json.parse("{\"service\":{\"debug\":null,\"port\":9090}}") else { return Error(()) }
let saved = copy base
let Ok(config) = std@1/json_merge_patch.apply(take base, patch, 8) else { return Error(()) }
// config is {"service":{"port":9090}}; saved still has port 8080 and debug true.
let Ok(other) = std@1/json_merge_patch.apply(copy saved, patch, 8) else { return Error(()) }
// patch remains usable, and saved remains unchanged.
```

Both complete inputs are preflighted using an explicit traversal stack before
mutation or replacement copying. Root is depth 0; every child, including scalars,
adds one. Equality with the cap succeeds. Empty root containers succeed at cap 0.
Deep replacement arrays and untouched target subtrees are checked too.
`Depth(max_depth)` publishes no partial result; the consumed target is dropped
normally. To retain the target on error, pass `copy target`. This is an explicit
copy, not a rollback guarantee. Merge evaluation uses explicit frames bounded by
the checked depth. Preflight re-walks indexed ancestor paths instead of copying
DOM subtrees, trading lookup work for depth-bounded bookkeeping. Ordinary JSON
map insertion/removal ordering applies; output is not canonicalized. Nested
object updates remove and reinsert the member.

## Prior art and adaptations

* `json-patch` at `1b0f707f9f4aad1009f0364f6ca73c5e88cdd85c`,
  `src/lib.rs:666–683`: object promotion, null deletion, child merge and whole-value
  replacement. MIT, copyright 2017 Ivan Dubrov; license beside this document.
* Its `specs/merge_tests.json:1–91` supplies all three named vectors;
  `tests/suite.rs:33–41` supplies the input/patch/expected runner structure.
* `json-merge-patch` at `e915ee199e7ccc65200b156ed2ca787f7503cba3`,
  `test/fixtures/index.json:1–190`: all fifteen independent Appendix A vectors.
  MIT, copyright 2015 Pierre Inglebert; fixture license beside this document.
  The JavaScript implementation is not used.

Hirsel adaptations: consume the target and return a Result; preflight both whole
inputs; use explicit traversal and merge stacks instead of recursive calls;
re-walk indexed ancestor paths during preflight to avoid copying JSON values
with the public borrowed APIs; make an explicit `unique` copy of the borrowed
patch only after preflight, then move its replacement values; use owned map
removal/insertion for nested targets and pending patch members, copying only
their next key; obtain an independent unique target after preflight when its
ownership is shared.

Vector JSON is represented as source strings parsed by the existing DOM codec.
A test-only comparator ignores object insertion order while retaining exact
number strings and array order. Added controls cover caps, 192-level arrays and
48-level object merges constructed without the parser, lexical numbers, literal
keys, patch reuse and external target copies. No language, IR, ABI or codec
changes.
