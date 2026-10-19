# hirsel-parser

Build the indexed AST and attach token/trivia ownership.

Allowed dependencies: hirsel-span, hirsel-intern, hirsel-syntax, hirsel-diag; shared QueryOutput in hirsel-types if desired.

Invariants and boundary:

- Consumes Vec<Token>, immutable SourceMap and FileContext; produces SyntaxFile plus recovery diagnostics.
- Does not call the lexer; the driver supplies its output. No name/type lookup (LANG-1/2).
- Preserve full tape even after parse errors. AST recovery nodes refer to original ranges.
- Use grammar newline rules and canonical block tails; signatures and bodyless declarations remain representable.
- Compute item DefPath from FileContext and declared name; hash owned source bytes. Duplicate detection belongs to resolution.

API: `parse(file, tokens, &context, &sources) -> ParseOutput` returns
`syntax: SyntaxFile` and ordered `diagnostics: Vec<Diagnostic>`. Keep the source
map alive for consumers. ABI strings, todo messages and literal
values retain their source spelling; decoding belongs to signature/body checking.
A keyword token used as a member keeps its Keyword kind; the parser interns its
spelling, as it does the names its desugarings introduce.

Parsing bounds constructed syntax height and nested grouping by
`MAX_PARSE_DEPTH` (1,024, spec #33). Bottom-up heights count every arena edge,
including binary/postfix nodes built in loops. Exhaustion sets
`ParseOutput.nesting_limit`; consumers must report an incomplete tooling outcome
and never publish its recovered tree. Deep direct callers use the driver compiler
thread boundary; ordinary recovery scans iteratively. Empty tapes, missing EOF and invalid spans
are handled without panics. Invalid lexical tokens remain on the
tape; lexer diagnostics should precede parser diagnostics in driver reports.
Item ownership plus the file-owned complement partitions the token tape. Final
orphan trivia/EOF remain in that complement; failed-item text is retained even
when no item AST can be constructed. Duplicate DefPaths remain unchanged for
resolution to diagnose.

`//:hirsel-parser-unittest` and `//:hirsel-parser-test-it` include nine structural snapshots,
executable v19 spec listings, grammar examples, newline/completion and recovery edge cases,
identity/trivia hashes, and 512 arbitrary token sequences. The test tokenizer
lives only in `tests/support`. `tools/bless.sh snapshots` updates structural snapshots.

The conformance gate parses every corpus file: each must parse cleanly unless its
header expects a syntax rejection. v20 field markers, annotated drop continuation,
noalloc_drop bounds and explicit empty type bounds are preserved without semantic
MEM-12 validation. Item.name_span and has_syntax_errors are
populated, including invalid lexical tokens in a recovered parse.

Revision 7 accepts represented opaque @capability ordinary/resource declarations instead of restricting the attribute to bodyless types. @serializable is a zero-argument type attribute and serializable a contextual generic bound. Eligibility, ceilings, structural bounds and scopes remain semantic checks; parser/formatter pins use syntax-only fragments.

Binding-level `mut` in a pattern reports LANG-13 with the accepted
`let mut (stream, _) = ...` spelling; mutability still applies to the whole pattern.

After LANG-14, a keyword used as a name is retained as a recovery name, including subsequent references. Its diagnostic names the keyword’s syntax role; the invalid file remains unpublished.

Conditional lets (LANG-13, `conditions.rs`) desugar here too: `if let P = e A else B`
becomes `match e { P => A, _ => B }` (`{}` without an else) and `'l: while let P = e A`
becomes `'l: while true { match e { P => A, _ => break } }`, recorded in
`SyntaxTree.conditions`. A scrutinee that is an unparenthesized `&&` or `||` is a
LANG-13 let chain and is rejected. Each `( e )` is recorded in `SyntaxTree.groups`.

F-strings (SEM-3, `formats.rs`) desugar here, in one place: the written pieces and
holes become one ordinary block binding `$fmt<depth>: List[u8] = []`, an
`Append(value, mut $fmt<depth>)` call per nonempty text piece and hole, and a tail
`Finish(take $fmt<depth>)`, whose callees are `ExprKind::Formatter` leaves. The
written form is recorded in `SyntaxTree.formats` for the formatter. Synthetic nodes
take the f-string's, the piece's or the hole's origin. A hole parses in statement
newline context and must close on the line its expression ends; an empty hole, a
missing `}` and an invalid format specifier are SEM-3. A specification of only
fill, alignment and width becomes a block `{ let $mark<depth> = Mark($fmt<depth>);
Append(value, mut $fmt<depth>); Pad(hole)($mark<depth>, fill, align, width,
mut $fmt<depth>) }`; any other becomes an `AppendSpec` call with six u64 option
arguments. A `name$` width or precision is a path at the spec token in place of
the constant; positional `N$` counts are SEM-3. Written spec spellings remain
alongside holes for formatting. `@format(path)` parses to
`AttributeArgument::Format`. Recovery treats Head and Tail pieces
as braces.

Loop labels are retained on for/while statements and optional break/continue targets. Only loops may be labelled; lexical target validation belongs to body checking.

`@test` is a zero-argument function-definition attribute; semantic LANG-9 signature validation belongs to signature normalization.
