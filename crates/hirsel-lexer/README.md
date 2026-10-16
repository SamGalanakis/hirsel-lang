# hirsel-lexer

Tokenize source while retaining all trivia.

Allowed dependencies: hirsel-span, hirsel-intern, hirsel-syntax, hirsel-diag; shared QueryOutput in hirsel-types if desired.

Invariants and boundary:

- Consumes FileId + SourceMap and mutable Interner; produces Vec<Token> plus diagnostics.
- Keep raw trivia/newlines and invalid tokens; no semicolon insertion (LANG-1).
- Longest-match punctuation and literal grammar follow docs/spec/syntax.md, including invalid suffixes (SEM-1/3).
- No parser, type information or filesystem assumptions; Invalid UTF-8 is SEM-3 at its original valid-prefix boundary; the driver rejects it before lexing. Oversize input is a tooling error.

Public API:

- `lex(file: FileId, sources: &SourceMap, interner: &mut Interner) -> LexOutput`;
  `LexOutput` owns `tokens: Vec<Token>` and `diagnostics: Vec<Diagnostic>`.
- Lexing never inserts semicolons or decides whether a statement is complete; a
  parser that needs physical LF/CRLF boundaries scans the token's source range.
- Every identifier and every emitted keyword spelling is interned. Keywords stay
  `Keyword` even after `.`; the parser can recover a member's `Symbol` from its
  immutable interner snapshot for spellings such as `x.type`. A keyword used as
  a declaration name is a parser LANG-14 error, not a lexer error. Literal spelling stays in the source span;
  numeric range/type checking and escape decoding belong to later phases.

Recovery and diagnostics:

- Each malformed numeric core/suffix becomes one `Invalid` token (SEM-1 for
  integers, SEM-2 for decimal float forms). Signs remain punctuation, including
  the magnitude of signed MIN. Numeric magnitudes are never parsed into machine
  integers in this phase.
- Strings/bytes recover through the closing quote, or stop immediately before a
  physical newline/EOF. A forbidden escaped newline recovers through its closing
  quote as one invalid token and one continuation diagnostic, retaining physical
  newline bytes. SEM-3 escape/scalar diagnostics point at the offending
  bytes; an unterminated literal points at its opening-to-end range. Every token
  with a lexical diagnostic becomes `Invalid`.
- Comments nest with an iterative depth counter. Invalid characters turn the
  complete comment into `Invalid`; unterminated block comments cite LANG-1.
  Raw Bidi_Control, bare CR, and an initial BOM cite SEM-3; unsupported source
  characters cite LANG-1. Diagnostic order follows discovery during scanning.
- An unavailable FileId returns an empty EOF tape and LANG-1 rather than panicking.
  Drivers validate UTF-8 and size before adding a source snapshot. Private scanner
  tests additionally exercise arbitrary invalid UTF-8 without losing any bytes.

Run `cargo test -p hirsel-lexer`; the conformance integration test prints its file
count and every reject whose first lexer diagnostic matches its rule and line
with `cargo test -p hirsel-lexer --test conformance -- --nocapture`.
