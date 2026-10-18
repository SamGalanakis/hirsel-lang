# hirsel-fmt

`format(&SyntaxFile, &SourceMap) -> FormatResult` prints the indexed AST. Names
come from the immutable SyntaxFile.symbols snapshot (including reserved members); literals
keep their spelling. `source` is the result and `unchanged` marks conservative
fallback. `format` uses SyntaxFile.has_syntax_errors, including diagnostics without
Invalid/Error nodes. External error-status and tape-spelling adapters have been retired.
Neither the library nor its normal dependencies import a parser or lexer.

Canonical layout (LANG-1/11/12; newline/precedence details follow grammar #21/#23):

- Indent two spaces. Use LF outside comments, preserve comment bytes exactly,
  and end each nonempty file with exactly one newline. Empty files stay empty.
- Separate top-level items by one blank line. No blank lines inside blocks.
  A resource's `drop` starts on the following line at the declaration's indent.
- Each declaration attribute gets its own line. Mirrored item/signature
  attribute metadata prints once. Function-type attributes are
  inline (`@noalloc fn`). Visibility precedes the declaration.
- Nonempty blocks, record declarations and match bodies always expand, with one
  statement/field/arm per line. Empty braces are `{}`. Records and match arms
  have trailing commas. Variant declarations stay on one line, with ` | `.
- Put one space around binary/assignment operators, `->`, `=>`, and `=`; ranges
  use `a..b`/`a..=b`. Prefix words have a following space; `-`, `!`, `~` do not.
  Calls, indices, projections and unmapped `?` are tight. Mapped `?` is ` ? E`.
- A colon has one following space. Commas have one following space in an inline
  list. Mode qualifiers are `T @ local(x) @ once`; attributes are `@noalloc`.
  Brands and placement use ` of .field` and ` in arena`/` in heap`.
- Parentheses/brackets around arguments, parameters, tuple entries, generic
  binders/arguments, patterns and local-mode paths fit on one line if their flat
  text fits in 88 Unicode-scalar columns at their start. Otherwise, break after
  the opening delimiter and between entries; indent entries two spaces and put
  the closing delimiter on its own line. Broken lists add no trailing comma
  (a one-element tuple always has one). Only delimited lists use width breaks;
  indivisible names, signatures, comments and other expressions may exceed 88.
- Results and `uses {a, b}` follow the parameters; closure authority moves before
  its body unless comments in the body/clause require keeping the trailing slot
  to preserve both attachment and comment order. Each contract starts on a new
  line indented two spaces from its declaration. Generated bodies use
  ` = comptime expression`. No inferred signature defaults are written out.
- Preserve tree grouping: lower-precedence operands are parenthesized, equal-
  precedence right operands are parenthesized, and comparisons/ranges group
  both nested equal-precedence operands. Group function/box types before outer
  qualifiers so those qualifiers cannot bind greedily to the result/inner type.
  Source expression/type grouping containing comments is retained to protect END.
  Group a path before a field projection (`(x).field`) so it stays a Field node
  rather than becoming a longer unresolved Path; group mapped `?` before a dot.
- A final expression **statement** keeps a semicolon to discard its value.
  Other statement terminators become newlines; a block tail has no semicolon.
  No width break is introduced before an ungrouped binary/postfix operator or
  between `?` and its mapping path.
- Comments remain in tape order at their next syntax boundary. Same-line
  comments stay after the preceding syntax; standalone comments and doc comments
  keep their own line, at the following syntax's indent. Nested/multiline comment
  contents are untouched. Top-level ownership follows `hirsel-syntax` exactly;
  unattached final trivia is printed after the final item. Extra source blank
  lines are collapsed to the policies above.
- Package heads stay adjacent (`json@1/`); AST path segments use dots and numeric
  type-size arguments use decimal. Grouping,
  separators and statement discards are generated from the AST. A conservative
  tape alignment ensures every other significant token is accounted for.

Safety boundary:

- Invalid tokens, any AST Error node, explicit parser error status, malformed or
  incomplete tapes, missing names/IDs, and unrepresented syntax return the whole
  original file, with `unchanged = true`. No partial rewrite loses tokens.
- Parsed trees satisfy the parser's 1,024-level syntax safeguard. Formatting
  runs on the driver compiler thread, as do deep direct test fixtures. The source
  ID must exist in the supplied SourceMap. No additional formatter depth limit
  changes a completed syntax judgment.
- Unit fixtures construct every surface tree form by hand. Parser/lexer may be
  **dev-dependencies only** for roundtrip/corpus oracles once those tasks land;
  driver composition still owns all production cross-phase imports.

The spec oracle builds the 19 executable file items of the overview in
docs/spec/index.md and the closure examples in docs/spec/closures.md by hand and
formats their source. Seven match exactly; 12 differ under the rules above. The
golden is `tests/fixtures/spec-overview.hir`.

- `normalize_tree`, `Percent`, `map`: collapse aligned trailing-comment padding.
- `reserve`, `sizes`: put the arrow beside the parameters, expand short blocks,
  and break lists containing multiline closure bodies inside their delimiters.
- `saxpy`: collapse its short parameter list, expand the loop block, and collapse
  trailing-comment padding.
- `Item`, `Order`, `AtomicFile`, `Header`, `Log`: expand records with trailing
  commas; `AtomicFile` also expands its drop body and aligns `drop` at column zero;
  `Header` places its attribute on a separate line.
- `save`: put the arrow beside the parameters and collapse comment padding.

The four schematic snippets (ellipsis `parse` and three top-level lets) are not
executable files under grammar/spec issue #25 and are excluded from this oracle.
Parser and lexer are dev-dependencies only. The conformance gate's `--check-format`
pass requires every corpus file to be its own canonical formatting.
`tests/corpus.rs` reparses the 19 retained spec snippets to identical trees
(ignoring trivia, spans and text hashes) with every comment in order, and checks
their canonical result against the hand-built goldens.

v20 prints field markers as ` @ growable`, drop annotations as `@noalloc drop(x)`,
and the contextual `noalloc_drop` bound. Omitted and explicit `: ()` bound clauses
remain distinct.

An f-string prints from `SyntaxTree.formats`: its piece spellings verbatim and each
hole as an ordinary expression, never the desugared block. A hole whose first token
is `{` is spaced (`f"{ {...} }"`) so it cannot read as the escape `{{`.

A conditional let prints from `SyntaxTree.conditions` as written, never the desugared
match or loop; a lazy boolean scrutinee keeps its parentheses. A let-else initializer
that ends in a block, ignoring parentheses, is printed parenthesized.

Each hole prints its retained spec after the expression, preserving Unicode fill.
