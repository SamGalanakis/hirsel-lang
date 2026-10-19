# hirsel-syntax

Own lossless lexical data, indexed ASTs, item paths and text hashes.

Allowed dependencies: hirsel-span, hirsel-intern, sha2; no phase implementation crates.

Invariants and boundary:

- IDs are u32 indices into the owning Vec; no node borrows another crate's storage.
- `DefId(NonZeroU32)` is the process-global session identity of a DefPath
  (`ids.rs`). Its table caches each path's fingerprint and Display form. Eq and
  Hash compare ids; Ord compares DefPaths, so a `BTreeMap<DefId, _>` iterates in
  path order under any id assignment. `DefMap<T>` is a `HashMap` keyed by id in
  unspecified order: sort before any output. DefPath stays the persisted,
  hashed, printed and mangled form. `salt_ids` (feature `id-salt`) is
  hirsel-intern's, re-exported.
- The token tape covers every source byte once, in order, plus empty EOF; comments and invalid tokens remain (LANG-1).
- Origin ranges cover significant syntax. Item.owned_tokens includes leading trivia and trailing same-line comments; the file owns leftover EOF trivia.
- A top-level item's text_hash hashes the exact bytes covered by owned_tokens, including attributes, contracts and drop; no newline normalization.
- DefPath uses package/version/module/item spellings supplied by FileContext; never spans, file names, hashes or Symbols (PROG-1/5).
  `DefPath::core` constructs core@1 from dot-separated module/item names;
  `is_core` compares the same identity without allocation. `package_is` matches
  both package name and version. `PackageId::name_is` explicitly ignores versions.
  Display is `package@version/module/item`, shared by diagnostics and tooling;
  binary fingerprints and the engine's module-only proof key remain separate.
- Duplicate paths are errors, not position-disambiguated identities. SyntaxFile roots preserve source order.
- Path/constructor classification, pattern refutability, labels and generic argument kinds need no parser type lookup (LANG-1/2).

Trivia attachment is deterministic: the first item takes initial trivia; between
items the preceding item takes any same-line trailing comment and its line break,
and the next item takes the remainder (including doc comments). Whitespace after
the last item stays with the file unless it belongs to a trailing same-line
comment. Formatting uses all tokens, not just owned item ranges. A resource's
`drop` and a function's attributes/contracts belong to the same item.

Canonical DefPath segments are nonempty, contain no separators, and use the
manifest's canonical package/version spellings. The resolver rejects duplicate
paths rather than appending positional ordinals. Moving an item between files
under the same manifest module leaves identity unchanged.

Revision 1 adds Item.name_span (exact declared name), FieldSyntax.growable and
DropSyntax.noalloc. Bound::NoallocDrop is contextual; no token/keyword was added.
Type declaration bounds are Option<Vec<Bound>>: None is omission, Some([]) is the
explicit `: ()` promise. Field growability is never a type qualifier. SyntaxFile
owns has_syntax_errors, which includes recovery
errors lacking AST Error nodes. Formatters must honor it. SourceMap ownership
still belongs to the driver. The parser follow-up implements the four new forms;
this revision adds transport without changing the conformance pending list.

Additive declaration-origin helpers recover exact parameter names/labels,
generic binder names, outer uses paths and variant names from the token tape.
Call them with the containing node's Origin and its own parameter/type IDs.
Nested function-type clauses and payload names stay in their own scopes; closure
trailing uses is part of that closure. No AST field or existing signature changed.
Absent tape/anonymous parameters return no precise site, preserving manual fixture
origins as the consumer fallback.

Revision 7 adds contextual Bound::Serializable. The existing Attribute row retains @serializable without a new keyword; represented opaque ordinary/resource capabilities reuse TypeBody and explicit bounds/authority/drop rows. Syntax conveys written facts, never schema or seal proof.

F-strings (SEM-3) add `TokenKind::Format(FormatPiece)`, `ExprKind::Formatter(FormatStep)`,
which only ever appears as the callee of a desugared step, and
`SyntaxTree.formats`: each written f-string's piece spellings and hole IDs, keyed by
the block the parser desugared it to.

Conditional lets (LANG-13) have no node either: `SyntaxTree.conditions` records each
written `if let` by its desugared match and each `while let` by its `while true` body
block, for the formatter and H's refutability judgment. `SyntaxTree.groups` keeps every
parenthesized expression with its span; `ends_in_block` reads it to accept
`let P = (if ..) else { .. }`.

`FormatPiece::Spec` retains the raw colon/options token. `FormatStep::AppendSpec`
carries parsed options; `FormatStep::Mark` and `FormatStep::Pad(hole)` pad a hole
whose options are only fill, alignment and width; `FormatString.specs` retains each
written spec. `AttributeArgument::Format` carries an `@format(function)` path.

Loop statements carry optional label Symbols; Break/Continue expressions carry optional target labels. StmtKind::loop_label exposes the written declaration without treating it as a value binding. A `Block` with a label is a labelled block expression (LANG-11), and `Break.value` is the operand of `break 'l value`, which only a labelled block accepts.
