# Syntax

This part is the normative grammar. It supplies syntax only, never type-system
permissions: a form that parses can still be rejected by a later rule. The rule ID beside
each production names the rule that owns it.

EBNF notation: quoted text is a terminal; `|` is choice, `{ X }` repetition, `[ X ]`
optional, parentheses group. `list(X)` is a comma-separated, possibly empty list with an
optional trailing comma; `nonempty(X)` is the same but needs one entry. Unquoted `NL`,
`END` and `EOF` are defined below. Productions operate on significant tokens except
where newlines or adjacency are mentioned. Spaces and comments may occur between tokens;
they may not split a token or a package/version prefix.

### LANG-1: Syntax

Hirsel is procedural, with value semantics: `fn`, `let`, `match`, module-qualified calls,
`todo` holes, `[T]` generics, `?`, sized numerics, tuples `(a, b)` with unit `()`,
`let mut`, assignment and compound assignment, `for`, `while`, `break`, `continue`, `if`,
contracts `requires`/`ensures`, value modifiers written before a type (`unique List[T]`,
`once fn() -> T`) and a brand and relation written after it (`T of p`, `T in x`).

Where only spelling differs from Rust, TypeScript or Python, the majority spelling wins;
`[T]` over `<T>` is the one exception, because angle brackets need type information to
parse. There are no methods, no `use`, no conditional operator and no test sublanguage.
The grammar is context-free and parseable without type information.

Source is UTF-8 with ASCII identifiers, nested comments, sized numeric suffixes and
scalar escapes; raw bidi controls are forbidden even in comments. A newline ends a
statement once it is complete; only the continuations the grammar names attach to the
next line. Semicolons separate statements explicitly. There is no expression hole spelled
`..`.

Malformed syntax with no more specific owner is LANG-1 (see
[primary sites](diagnostics.md#primary-sites) for the declaration components owned by
their own rules).

Canonical formatting preserves the parsed structure and every comment and is idempotent.
The lossless token tape reconstructs the original bytes; canonical formatter output need
not be byte-identical to its input.

A parser may bound nesting depth as an implementation resource (the reference parser
allows 1,024 constructed levels, explicit grouping included). Exceeding such a bound is an
incomplete tooling outcome, never LANG-1 and never a clean partial syntax tree.

### LANG-14: Keywords

The reserved keywords are exactly:

```text
fn let mut match if else for in while break continue return take copy unique
type distinct flat resource drop box pub opaque const comptime shape heap
before requires ensures result where uses of local global once todo extern
unsafe true false
```

`take`, `mut`, `copy` and `unique` are prefix forms (`take x`, `copy x`). In type position
`unique`, `once` and the contextual name `sendable` are prefix modifiers
(`unique List[T]`); `sendable` is a modifier only when a type follows it. `in` also
introduces a relation (`T in x`), where the contextual name `env` denotes a callable's
environment. `local` and `global` are reserved keywords but are not type qualifiers.

A keyword cannot be declared as a name. After `.`, a keyword may be a member spelling.

## Lexical grammar

Input is UTF-8, with no byte order mark; invalid UTF-8 is rejected before any position is
assigned. Offsets are bytes. LF and CRLF are newlines; a bare CR is rejected. No Unicode
normalization occurs. The raw characters U+061C, U+200E–U+200F, U+202A–U+202E and
U+2066–U+2069 (Unicode `Bidi_Control`) are rejected everywhere, comments included; a
string escape can represent them. Columns are one-based Unicode scalar counts; a tab
counts one column ([SEM-3](semantics.md#sem-3-strings)).

```ebnf
letter       = "A".."Z" | "a".."z" | "_" ;                 (* LANG-4/14 *)
digit        = "0".."9" ;                                  (* SEM-1 *)
identifier   = letter, { letter | digit } ;                (* LANG-4 *)
space        = " " | "\t" ;                                (* LANG-1 *)
NL           = LF | CR, LF ;                               (* LANG-1 *)
line-comment = "//", { non-newline-scalar } ;              (* LANG-1 *)
doc-line     = "///", { non-newline-scalar } ;             (* LANG-1 *)
block-comment= "/*", { block-comment | non-delimiter-scalar }, "*/" ; (* LANG-1 *)
doc-block    = "/**/" | "/**", { block-comment | non-delimiter-scalar }, "*/" ; (* LANG-1 *)
```

`non-newline-scalar` excludes CR, LF and the forbidden controls. `non-delimiter-scalar` is
any allowed scalar except at a position beginning `/*` or `*/`; comments nest. `/**/` is
an empty doc block and `////` is a doc line. A block comment containing newlines
contributes those newlines to the termination rule. Doc comments attach documentation and
otherwise behave as comments. An unterminated comment is one invalid token and a LANG-1
diagnostic.

`plain data key sendable inert copyable noalloc_drop serializable size` are contextual
names in bounds; `sendable` is also a contextual type modifier when a type follows it,
and `env` is a contextual place name inside a relation. Attribute names (`noalloc`,
`layout`, `packed`, `align`, `soa`, `blocking`, `incomplete`, `capability`,
`serializable`, `loop_source`, `format`, `advance_only`, `preserves_captures`, `preserves_unique`,
`captures`) are
contextual. `c`, `ffi`, primitive types and library types and functions are identifiers.
`_` is the discard pattern, not a value binding.

```ebnf
digits(D)    = D, { [ "_" ], D } ;                          (* SEM-1 *)
integer-core = digits(digit)
             | "0x", digits(hex-digit)
             | "0o", digits("0".."7")
             | "0b", digits("0" | "1") ;                   (* SEM-1 *)
hex-digit    = digit | "a".."f" | "A".."F" ;               (* SEM-1 *)
int-suffix   = "u8" | "u16" | "u32" | "u64" | "u128"
             | "i8" | "i16" | "i32" | "i64" | "i128" ;     (* LANG-1, SEM-1 *)
float-suffix = "f32" | "f64" ;                             (* SEM-2 *)
exponent     = ("e" | "E"), [ "+" | "-" ], digits(digit) ; (* SEM-2 *)
integer      = integer-core, [ int-suffix ] ;              (* LANG-2, SEM-1 *)
float        = digits(digit), ".", digits(digit), [ exponent ], [ float-suffix ]
             | digits(digit), exponent, [ float-suffix ]
             | digits(digit), float-suffix ;               (* LANG-2, SEM-2 *)
escape       = "\\", ( "\\" | '"' | "n" | "r" | "t" | "0" )
             | "\\x", hex-digit, hex-digit
             | "\\u{", hex-digit, { hex-digit }, "}" ;     (* SEM-3 *)
raw-string   = "r", hashes, '"', raw-body, '"', hashes ; (* SEM-3 *)
raw-bytes    = "br", hashes, '"', ascii-raw-body, '"', hashes ; (* SEM-3 *)
hashes       = { "#" } ;
string       = '"', { string-scalar | escape }, '"' ;      (* SEM-3 *)
bytes        = 'b"', { ascii-string-scalar | byte-escape }, '"' ; (* SEM-3 *)
byte-escape  = "\\", ( "\\" | '"' | "n" | "r" | "t" | "0" )
             | "\\x", hex-digit, hex-digit ;               (* SEM-3 *)
character    = "'", ( character-scalar | character-escape ), "'" ;
character-escape = "\\", ( "n" | "t" | "r" | "0" | "\\" | "'" )
                 | "\\u{", hex-digit, { hex-digit }, "}" ;
byte         = "b'", ( ascii-character-scalar | character-escape
                     | "\\x", hex-digit, hex-digit ), "'" ;     (* LANG-1 *)
literal      = integer | float | character | byte | string | bytes | raw-string | raw-bytes | "true" | "false" ; (* LANG-2, SEM-1/2/3 *)
f-string     = 'f"', f-text, { "{", expression, [ ":", format-spec ], "}", f-text }, '"' ; (* SEM-3 *)
f-text       = { f-scalar | escape | "{{" | "}}" } ;      (* SEM-3 *)
format-spec  = [ [ fill ], align ], [ "+" | "-" ], [ "#" ], [ "0" ],
               [ count ], [ ".", count ], [ "x" | "X" | "b" | "o" ] ;
count        = digit, { digit } | name, "$" ;           (* SEM-3 *)
align        = "<" | ">" | "^" ;
fill         = string-scalar ; (* one scalar, excluding braces *)
```

`string-scalar` excludes the quote, backslash, raw controls U+0000–U+001F and U+007F,
newlines and `Bidi_Control`; `ascii-string-scalar` also excludes non-ASCII. A string
`\xNN` is limited to 00–7F, so no escape produces invalid UTF-8; bytes permit 00–FF.
`\u{...}` has one to six hex digits and must denote a Unicode scalar value (no surrogates,
nothing above 10FFFF). A character literal denotes exactly one Unicode scalar;
`character-scalar` excludes apostrophe, backslash, raw controls U+0000–U+001F
and U+007F, newlines and `Bidi_Control`. Its Unicode escape has the same one-to-six
hex-digit scalar restriction. Malformed, unterminated, empty or multi-scalar
character literals are LANG-1. Character literals have no suffix. A byte literal
(`b'a'`, `b'\n'`, `b'\x7f'`) is a character literal of one ASCII scalar prefixed by
`b`: `ascii-character-scalar` is a `character-scalar` below U+0080, and its escapes,
including `\xNN` and `\u{...}`, must denote a value from 00 to 7F. A byte literal
that is empty, has more than one scalar, or denotes a non-ASCII value is LANG-1.
`f-scalar` is a `string-scalar` other than `{` and `}`.
Raw String literals (`r"text"`, `r#"a "quote""#`) and raw Bytes literals
(`br"text"`, `br##"a "# quote"##`) close at the first quote followed by the
opening delimiter's number of hashes. Any number of hashes is permitted. Their
bodies preserve every source byte, including LF newlines and tabs; there are no
escapes, interpolation or indentation stripping. CR, including CRLF inside a raw
literal, is LANG-1. Outside raw literals the existing LF/CRLF source policy applies.
Other raw controls, DEL, Bidi_Control and invalid UTF-8 retain ordinary string
restrictions (SEM-3); raw Bytes additionally require ASCII source scalars.
The formatter preserves the complete literal spelling verbatim.
There are no escape line continuations or triple-quoted strings, hexadecimal floats, leading or trailing decimal
points, or underscores immediately before a suffix.

An f-string (`f"total: {n} of {list.len(xs)}\n"`, [SEM-3](semantics.md#sem-3-strings))
is a string with holes. `f"` is one token start, like `b"`: `f "x"` is a name and a
string. Its text follows string rules, with `{{` and `}}` for literal braces; a lone `}`
in the text is SEM-3. Each hole holds one expression, which may contain string literals
and f-strings of its own. The lexer tapes an f-string as pieces around ordinary hole
tokens: `f"text{`, `}text{`, `}text"`, or a single `f"text"` with no holes, so a hole's
closing `}` is the first `}` that closes no brace the hole opened. A hole is not a
grouping: its expression continues onto another line only where it would outside the
f-string (an open delimiter or a missing operand), and its closing `}` stands on the line
where the expression ends. A colon after the expression introduces a format
specification, ending at `}`; its spelling is retained as one token.
An empty hole, a hole without its `}`, an invalid specification or an unsupported
format type for the hole's value is SEM-3. A width or precision is a decimal u64
constant or `name$`, naming a `u64` in scope (`{x:w$}`, `{x:.p$}`). See
[the formatting protocol](std-library.md#the-formatting-protocol) for semantics.

Integers have no sign: `-128i8` is unary minus applied to a magnitude token, and the
checker accepts the magnitude of `MIN` there. An adjacent malformed suffix or digit
sequence is one invalid numeric token, not a number followed by a name. Literals keep
their spelling; a suffix fixes the expected type, and an unsuffixed literal does not
default ([LANG-2](language.md#lang-2-prefix-law)).

Punctuation uses longest match:

```text
( ) [ ] { } , : ; . .. ..= @ ? -> =>
+ - * / % ! ~ & | ^ && || << >> = == != < <= > >=
+= -= *= /= %= &= |= ^= <<= >>=
```

`1..2` is integer, range, integer: a fractional dot must be followed by a digit. `//` and
`/*` start comments before `/` is considered an operator. `b"..."` is one bytes token.
Reserved words match whole identifiers only. The token tape keeps spaces, comments,
physical newlines, invalid tokens and a zero-width EOF token, so every input byte can be
reconstructed.

## Statements and newlines

The lexer inserts no semicolons. The parser applies these ordered rules, using syntax
only (LANG-1, LANG-11):

1. A newline inside a parenthesized or bracketed list, or a grouped expression or type,
   is whitespace. A nested `{ ... }` block establishes its own statement context: outer
   call parentheses do not suppress newlines inside a closure body. Record and variant
   declaration braces and `uses` braces are lists, not blocks.
2. At file and block level, leading blank and comment-only lines are discarded. Tokens
   are consumed until a statement or item can be complete. While a required operand,
   delimiter, pattern, type, assignment right-hand side or control or function body is
   absent, a newline is whitespace, so `let x =\n f()` and `a +\n b` continue.
3. At a complete statement or item, a newline is `END`. The next line's leading operator,
   `.`, `(`, `[` or `?` never reopens it; wrap such an expression in parentheses to
   continue it. A bare `return`, `break` or `continue` is complete at a newline:
   `return\n x` does not return `x`.
4. These continuations take priority over rule 3, across blank lines and comments: `else`
   belongs to the nearest unmatched `if`; `drop` or `@noalloc drop` belongs to the
   preceding resource declaration; `->`, `uses`, `requires`, `ensures`, `=` and a
   body-opening `{` continue a function declaration that has no body yet; `uses`
   immediately after a closure block belongs to that closure; a declaration's attributes
   and visibility wait for the declaration; `where` continues a distinct declaration. No
   other lookahead suppresses an `END`.
5. A `?` takes a mapping path only when the path starts on the same physical line, even
   inside parentheses: `f()?\n C` is not `f()? C`.
6. `;` is an explicit `END` in blocks and files. `}` and EOF end a complete pending
   statement without being consumed as its `END`. A completed block-valued expression
   (`if`, `match`, a block), a local `fn`, `while` or `for` may be followed by another
   statement on the same line; other adjacent statements need a newline or `;`.
7. The last expression of a block is its value even when followed by newlines; a trailing
   semicolon discards it. A final `let`, assignment, loop or local `fn` makes the block's
   value unit. A final `return` parses and is rejected by
   [LANG-11](language.md#lang-11-blocks).

In list braces (type fields, match arms) a newline can replace a comma after a complete
entry; after a comma it is whitespace. Tuple, call and generic lists always need commas.
The newline before a binary operator ends a complete match-arm expression just as it ends
an expression statement. Indentation is never significant.

```ebnf
END          = NL, { NL } | ";" | implicit-end ;           (* LANG-1/11 *)
entry-sep    = ",", { NL } | NL, { NL } ;                  (* LANG-1/12 *)
```

`implicit-end` is the zero-width boundary of rule 6.

```hirsel
fn total(a: u64, b: u64) -> u64 {
  let sum = a +
    b                    // the operator is not complete: the newline is whitespace
  let doubled = (sum
    * 2)                 // grouping continues across the newline
  doubled
}
```

## Names, files and declarations

```ebnf
name         = identifier ;                              (* LANG-4 *)
member       = identifier | keyword ;                    (* LANG-4 *)
version      = integer-core ;                            (* PROG-1 *)
package-head = name, "@", version, "/" ;                 (* LANG-4, PROG-1 *)
value-path   = [ package-head, { name, "/" } ], name, { ".", member } ; (* LANG-4 *)
type-path    = [ package-head ], name, { ("/" | "."), member } ; (* PROG-1 *)
brand-path   = [ "." ], value-path ;                     (* PROV-1/5 *)
file         = { NL | item, END }, EOF ;                 (* LANG-6, PROG-1 *)
visibility   = "pub", [ "opaque" ] ;                     (* PROG-2 *)
item         = { attribute | analysis }, [ visibility ],
               ( function | const-def | type-def ) ;     (* LANG-6, PROG-2 *)
attribute    = "@", ( "test" | "noalloc" | "soa" | "packed" | "blocking" | "incomplete"
             | "serializable", [ "(", "check", "=", value-path, ")" ]
             | "layout", "(", "c", ")"
             | "align", "(", integer, ")"
             | "capability", "(", ("global" | "local"), ")"
             | "loop_source", "(", value-path, ",", value-path, ")"
             | "format", "(", value-path, ")"
             | state-attribute ) ;      (* LANG-9, MEM-5/7, CONC-2, MODE-6, CAP-5, SER-1, SEM-3/4, CLO-2 *)
analysis     = "@", "analysis", "(", name, { ".", name },
               [ ":", payload-token, { payload-token } ], ")" ; (* PS-17 *)
payload-token = any token but an unbalanced "(", ")", "[", "]", "{" or "}" ; (* PS-17 *)
state-attribute = ("advance_only" | "preserves_captures" | "preserves_unique" | "captures"),
                  "(", nonempty(value-path), ")" ; (* CLO-2; on a type, `captures` names fields: CAP-2, CLO-4 *)
type-state-attribute = ("advance_only" | "preserves_captures" | "captures"),
                  "(", nonempty(value-path), ")" ; (* CLO-2, LANG-7 *)
function     = [ "extern", string ], "fn", name, fn-header,
               [ block | "=", "comptime", expression ] ; (* LANG-6/10, PROG-4, CT-1 *)
fn-header    = [ generics ], "(", list(parameter), ")",
               [ "->", type ], [ uses ], { contract } ;  (* LANG-6/12, FACT-1, CLO-4 *)
parameter    = { analysis }, [ name ], [ passing ], name, ":", type ; (* LANG-12, OWN-1/2/3, PS-17 *)
passing      = "take" | "mut" ;                          (* OWN-1/2/3 *)
const-def    = "const", name, ":", type, "=", expression ; (* LANG-6, CT-1 *)
generics     = "[", nonempty(generic), "]" ;             (* LANG-7, GEN-1 *)
generic      = name, [ ":", ( "size" | bounds ) ] ;      (* LANG-7, GEN-1 *)
bounds       = bound, { "+", bound } ;                   (* GEN-1 *)
bound        = "plain" | "flat" | "data" | "key" | "sendable" | "inert" | "copyable"
             | "noalloc_drop" | "serializable" ;         (* GEN-1, MEM-5, SER-1 *)
uses         = "uses", "{", list(value-path), "}" ;      (* CAP-2/3, CLO-2/4 *)
contract     = ( "requires" | "ensures" ), expression ;  (* FACT-1 *)
type-def     = [ "flat" | "resource" ], "type", name, [ generics ],
               [ ":", type-bounds ], [ uses ],
               ( record-fields | "=", type-rhs | opaque-end ), [ drop-def ]
             | "distinct", "type", name, [ generics ], "(", { analysis }, name, ":", part-type, ")",
               [ "where", expression ] ;  (* LANG-7, GEN-1, CAP-5, SER-1, MEM-12, DROP-2, FACT-2 *)
type-bounds  = bounds | "(", ")" ;                       (* LANG-6, GEN-1, PROG-2 *)
record-fields= "{", [ { analysis }, field, { entry-sep, { analysis }, field }, [ entry-sep ] ], "}" ; (* LANG-7/12, PS-17 *)
field        = name, ":", part-type, [ "@", "growable" ] ; (* LANG-12, PROV-5, MEM-12 *)
type-rhs     = variant, "|", variant, { "|", variant }
             | name, "(", list(payload), ")"
             | type ;                                    (* LANG-7 *)
variant      = name, [ "(", list(payload), ")" ] ;       (* LANG-7/12 *)
payload      = { analysis }, [ name, ":" ], part-type ; (* LANG-12, PS-17 *)
opaque-end   = empty ;                                   (* PROG-2 *)
drop-def     = [ "@", "noalloc" ], "drop", "(", name, ")", block ; (* DROP-2/3 *)
```

An `analysis` annotation ([PS-17](proof-seam.md#ps-17-analysis-annotations)) is
meaning-free: it is allowed before a parameter only in a top-level function definition, and
nowhere but the positions above. `analysis` is the one attribute name an analysis outside
the compiler owns.

Only an opaque declaration can use `opaque-end`. `drop` requires `resource` and belongs to
that item, including when it starts the next line with `@noalloc`.

A record field, variant payload, distinct field, tuple element, `box` payload or type
argument is a **part type**: it takes modifiers and a brand but no relation, because a
part is as local as its owner and its current content
([OWN-8](ownership.md#own-8-parts)). The parser accepts a written relation there for
recovery and reports LANG-7 at that type; a relation reaching a part through an alias or
grouping is the same LANG-7 at its annotation. A function type in a field keeps its own
result relation (`f: fn() -> T in env` relates the result), and a parenthesized single
type is not a tuple. A type argument transports its actual's relation; it is never
written.

`@ growable` ends a field type: it is field metadata, not a type qualifier. It parses on
any record field and is validated under [MEM-12](memory.md#mem-12-flat-types). A single
bare right-hand-side name is an alias, not a one-variant type; spell that `Only()`.
Payloads are uniformly labelled or uniformly positional. `pub opaque` applies only to
types.

`@capability` applies to opaque ordinary or resource declarations with a record, variant
or alias representation, or to a bodyless signature fragment, and states global or
scoped ([CAP-5](capabilities.md#cap-5-capability-sealing)). `@serializable` is an
attribute on transparent records, variants, unconstrained distinct types and flat types,
and, as `@serializable(check = f)`, on an opaque record or variant whose decoded
representations `f` admits; `[T: serializable]` is a generic schema bound
([SER-1](serialization.md#ser-1-serializable-schemas)). Neither is a prefix value
modifier. Attributes have placement restrictions: layout attributes on types; `noalloc`
on functions, function types and `drop`; `test` only on ordinary function definitions
with the LANG-9 signature; `loop_source` and `format` on nominal types; state attributes
on functions and callable types; `blocking` on externs; `incomplete` on functions. State
selectors name only that callable's parameters and their explicit field paths; an omitted
attribute promises nothing.

A parameter's first name is a call label: `into mut xs: List[T]`, `from src: Path`. A
parameter without one cannot be labelled at a call. Paths in `uses` are not types; name
resolution decides whether each names a capability type or a parameter. A package head
such as `json@1/` is written without spaces. In expression context an unversioned `a/b`
is always division; name another package's definitions with a versioned path or an
exposed module head. Type-context paths may contain `/` directly. Module and package
assignment is launch input ([PROG-1](programs.md#prog-1-names)), not a source form.
`unsafe ffi` is package metadata, never an expression block.

Bodyless function declarations parse as signature fragments; execution needs an
implementation ([LANG-6](language.md#lang-6-signatures)). A definition containing `todo`
must be marked `@incomplete`; this is checked, never inferred into its signature.

Opaque type declarations write both a bound clause and `uses`. The empty bound clause
`: ()` states no exported bounds; omitting the clause on an opaque export is a LANG-6
error. Nonopaque types may omit it, and their bounds are computed from their declared
fields. A resource's destruction authority is the union of its field summaries and its
explicit type-level `uses`; no drop body is read to compute it.

## Types

```ebnf
type         = { modifier }, type-atom, [ brand ], [ relation ] ; (* LANG-1/6/7, MODE-1/7, PROV-1/4 *)
type-atom    = type-path, [ "[", nonempty(type-argument), "]" ]
             | "(", ")" | "(", type, ")"
             | "(", part-type, ",", list(part-type), ")"
             | "box", part-type
             | { "@", ("noalloc" | type-state-attribute) }, "fn", "(", list(type-parameter), ")",
               [ "->", type ], [ uses ] ;                (* LANG-7, MEM-5/7, CLO-1/4 *)
type-argument= part-type | integer ;                     (* LANG-7, GEN-1 *)
part-type    = { modifier }, type-atom, [ brand ] ;      (* no relation: LANG-7, OWN-8 *)
type-parameter= parameter | [ passing ], type ;          (* CLO-1, OWN-1/2/3 *)
modifier     = "unique" | "once" | "sendable" ;          (* MODE-2/3/4; sendable contextual *)
brand        = "of", brand-path ;                        (* PROV-1/2/5 *)
relation     = "in", ( "heap" | relation-place
                     | "{", nonempty(relation-place), "}" ) ; (* MODE-1, PROV-4 *)
relation-place= value-path ;            (* parameter, field path, binding, arena or env *)
```

Modifiers are prefix and cover the whole type that follows, a function type's result
included: `once fn() -> Bytes in env` is a once function whose result is `in env`, and
`fn() -> unique T` makes only the result unique. A brand and a relation follow the type,
in that order, and bind to the nearest type on their left. A function type's result is a
full `type`, so `fn() -> T in x` relates `T`; a function type ends at its `uses` clause, so
`fn() -> T uses {} in x` relates the function value. Only a function value whose type has
no `uses` clause needs parentheses: `(fn() -> T) in x`. The one rule to remember:
**modifiers go before and cover everything after them; `of` and `in` go after and bind to
the nearest type before them.**

`heap` stands alone (`in heap` is the empty relation) and cannot appear in a braced set.
Repeating a modifier, or writing one of the removed qualifiers `@ unique`, `@ once`,
`@ sendable`, `@ local` or `@ global`, is LANG-7 at the type; the parser recognises the
`@` forms only to report them. `@noalloc fn(...) -> ...` states a function type's
allocation contract.

Names used as type arguments parse identically whether they resolve to a type parameter
or a size parameter. Size literals are unsuffixed nonnegative `u64` values; there is no
size arithmetic. No type argument appears at a call site: `f[T]` in an expression is
always indexing.

A relation names places: parameters and their field paths (`in p.a`), bindings in scope,
arenas, and `env`, the environment of a closure or function type. Which places are valid
is checked by [MODE-1](ownership.md#mode-1-locality) and
[PROV-4](brands.md#prov-4-arena-placement); the parser accepts any value path.

`Packed[T]`, `Inline[T, n]`, `Simd[T, n]`, `Shared[T]`, `Region[T]`, `Slots[T]`,
`SlotId[T]`, `Task[T]`, `Sender[T]`, `Receiver[T]`, `Atomic[T]`, `Iter[T]`, `FnPtr[...]`,
`Ptr[T]` and the ordinary containers are named type applications; their layout and
sendability rules are semantic. `String` and `Bytes` are primitives; unit is `()`, and a
tuple type needs a comma except for unit. Raw pointers are `Ptr[T]`; their operations take
`Unsafe` through ordinary functions.

## Expressions, statements and patterns

```ebnf
block        = "{", { statement, END }, [ expression, [ END ] ], "}" ; (* LANG-1/11 *)
statement    = let-stmt | assignment | for-stmt | while-stmt | local-fn | expression ; (* LANG-1/10/11 *)
let-stmt     = "let", [ "mut" ], pattern, [ ":", type ], "=", expression,
               [ "else", block ] ;                       (* LANG-5/13/15 *)
assignment   = expression, assign-op, expression ;       (* LANG-5 *)
assign-op    = "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "<<=" | ">>=" | "&=" | "|=" | "^=" ; (* LANG-1, SEM-1 *)
loop-label   = "'", name ; (* one token; separate from character literals *)
for-stmt     = [ loop-label, ":" ], "for", pattern, "in", expression, block ; (* SEM-4, LANG-13 *)
while-stmt   = [ loop-label, ":" ], "while", ( expression | let-cond ), block ; (* LANG-1/13 *)
let-cond     = "let", pattern, "=", expression ;         (* LANG-13 *)
local-fn     = "fn", name, fn-header, block ;            (* LANG-10 *)
expression   = range ;                                   (* LANG-1 *)
range        = logical-or, [ (".." | "..="), logical-or ] ; (* SEM-4 *)
logical-or   = logical-and, { "||", logical-and } ;      (* LANG-1 *)
logical-and  = comparison, { "&&", comparison } ;        (* LANG-1 *)
comparison   = bit-or, [ ("==" | "!=" | "<" | "<=" | ">" | ">="), bit-or ] ; (* GEN-1, SEM-1/2 *)
bit-or       = bit-xor, { "|", bit-xor } ;               (* SEM-1 *)
bit-xor      = bit-and, { "^", bit-and } ;               (* SEM-1 *)
bit-and      = shift, { "&", shift } ;                   (* SEM-1 *)
shift        = additive, { ("<<" | ">>"), additive } ;   (* SEM-1 *)
additive     = product, { ("+" | "-"), product } ;       (* SEM-1/2 *)
product      = prefix, { ("*" | "/" | "%"), prefix } ;   (* SEM-1/2 *)
prefix       = ("-" | "!" | "~" | "take" | "mut" | "copy" | "unique" | "box" | "comptime"), prefix
             | postfix ;                                 (* LANG-14, OWN-1/8, MEM-7, CT-1 *)
postfix      = primary, { "(", list(argument), ")" | "[", expression, "]"
             | ".", member | "?", [ same-line-value-path ] } ; (* LANG-2/8/12 *)
argument     = [ name, ":" ], expression ;               (* LANG-12 *)
primary      = literal | f-string | value-path | "heap" | "result"
             | "(", ")" | "(", expression, ")"
             | "(", expression, ",", list(expression), ")"
             | "[", list(expression), "]"
             | "[", expression, ";", expression, "]"
             | block | loop-label, ":", block | if-expr | match-expr | closure
             | "shape", "(", type, ")"
             | "before", "(", expression, ")"
             | "todo", [ "(", string, ")" ]
             | "return", [ expression ] | "break", [ loop-label, [ expression ] ]
             | "continue", [ loop-label ] ; (* LANG-1/2/11, MEM-4, FACT-1, CT-3 *)
if-expr      = "if", ( expression | let-cond ), block, [ "else", (block | if-expr) ] ; (* LANG-1/13 *)
match-expr   = "match", expression, "{",
               [ arm, { entry-sep, arm }, [ entry-sep ] ], "}" ; (* LANG-7/13 *)
arm          = pattern, [ "if", expression ], "=>", expression ; (* LANG-7/13 *)
closure      = "fn", "(", list(parameter), ")", [ "->", type ],
               [ uses ], { contract }, block, [ uses ] ; (* CLO-1/4, FACT-1 *)
pattern      = alternative, { "|", alternative } ;      (* LANG-13 *)
alternative  = "_" | name | scalar, [ "..=", scalar ]
             | "(", ")" | "(", pattern, ")"
             | "(", pattern, ",", list(pattern), ")"
             | name, "(", pattern-fields, ")" ;          (* LANG-7/13 *)
pattern-fields= empty | "..", [ "," ]
             | pattern-field, { ",", pattern-field }, [ ",", [ "..", [ "," ] ] ] ; (* OWN-8, LANG-13 *)
pattern-field= [ name, ":" ], pattern ;                  (* LANG-12/13 *)
scalar       = literal | "-", integer ;                  (* LANG-13 *)
```

`keyword` in `member` means any LANG-14 token; `same-line-value-path` is a `value-path`
subject to newline rule 5. `empty` is the empty sequence.

`for` patterns must be irrefutable. Match guards are pure boolean expressions and do not
count toward exhaustiveness. `let` rejects refutable patterns semantically, including a
bare name that resolves to a nullary variant; parsing never guesses from capitalisation.
Labels and payload arity are checked against the expected type. A record rest pattern is
last and needs a comma after preceding fields (`Rec(x: x, ..)`). A range pattern is
closed (`a..=b`) and its bounds are literals. A parenthesized alternation inside an
alternation is the same alternation (`(A | B) | C` is `A | B | C`). A `let` with `else` takes a refutable
pattern; its initializer cannot end in a block (a labelled block included) unless parenthesized. A `let-cond` takes a
refutable pattern, and its expression cannot be an unparenthesized `&&` or `||` (no let
chains). There is no qualified variant constructor.

A statement expression becomes the block's tail exactly when it is last and not followed
by a semicolon. An assignment's left side parses as an expression and must check as a
mutable place. `return` consumes the rest of the current expression and binds below
range. A loop label such as `'outer` may
prefix a `for` or `while` statement (`'outer: for x in xs { ... }`) or a block
(`'outer: { ... }`, LANG-11), and `break 'outer` or `continue 'outer` selects
that enclosing loop or block. Only a labelled exit to a labelled block carries a
value: `break 'outer value`, whose value consumes the rest of the expression as
`return`'s does; a value break naming a loop is LANG-1. Unlabelled
exits select the innermost loop. Labels occupy a separate lexical namespace,
are visible within their loop (including a while condition, excluding a for
source expression), and cannot shadow an active label in the same function.
An unknown, out-of-scope or shadowed label is LANG-7. A closure or local
function starts a new label scope; an exit cannot cross a function boundary.
A label name is an ordinary non-keyword name, excluding `_`; a closing
apostrophe belongs to a character literal instead. A bare exit ends at a
statement newline, so its label, and a break value, must start on the same logical line. Labelled
exits end the same loans and clean up every exited scope in the same order
as an unlabelled exit to that loop; a continue retains the selected loop's
iteration preparation. Only loops and blocks may be labelled. Constructors use ordinary call
syntax; field and variant names are resolved against the expected type. `[a, b]` is a
sequence literal whose kind (`Array`, `List`, `Inline`) comes from a usable complete
expected type. With no expected type, an ordinary nonempty literal defaults to
core `List[T]` only when its first element alone supplies a complete, valid,
non-bottom T; subsequent elements are checked once against T, in order
([LANG-2](language.md#lang-2-prefix-law)). A user type named `List` does not change
that default. Empty literals still need an expected sequence type. `[e; N]` repeats a value in an expected
`Array[T]` or `Inline[T, n]`: `N` is a definition-time-known `u64` expression,
and for `Inline` it must equal `n`. The element is evaluated once, including
when `N` is zero; a nonconstant element must be copyable
([LANG-7](language.md#lang-7-types)). `box e` builds the block a `box T` needs. The
`= comptime expression` function body takes a generated body
([CT-5](compile-time.md#ct-5-generation)); `shape(T)` reads a type's shape at
compile time ([CT-3](compile-time.md#ct-3-shapes)). `comptime e` has prefix precedence; parenthesize a
larger operand.

A closure literal may omit its result type, which is then inferred inside the enclosing
body; a declared function or function type with no arrow returns unit. A closure may
write `uses` before or after its block, not both. There is no capture syntax: scoped
captures borrow, escaping captures copy or take by use, and `take x` inside a closure
selects a taken capture at creation ([CLO-4](closures.md#clo-4-escaping)). There are no
task, transaction, sandbox, arena, mutex or channel statements: all are library calls
taking ordinary function arguments.

### Precedence

Strongest first:

| Operators | Associativity | Rules |
|---|---|---|
| call, `.`, index, `?` and `? C` | left, postfix | LANG-2/8/12 |
| `- ! ~ take mut copy unique box comptime` | right, prefix | LANG-14, SEM-1, CT-1 |
| `* / %` | left | SEM-1/2 |
| `+ -` | left | SEM-1/2 |
| `<< >>` | left | SEM-1 |
| `&`, then `^`, then `\|` | left at each level | SEM-1 |
| `== != < <= > >=` | non-associative | GEN-1, SEM-1/2 |
| `&&`, then `\|\|` | left at each level, short-circuit | LANG-3 |
| `.. ..=` | non-associative, both operands required | SEM-4 |
| `return e` | consumes a complete expression | LANG-11 |
| assignment and compound assignment | statement only, no chaining | LANG-5 |

Comparisons do not chain, and ranges do not nest without parentheses. `? C` consumes only
the mapper path, so `f()? C(x)` calls the propagated success value with `x`. Evaluation is
strictly left to right even where precedence groups to the right; a short-circuit
right-hand side or an untaken branch is not evaluated.

## Why parsing needs no type information

Types occur only after a parameter, field or binding `:`, after `->`, in generic
declarations, inside a type's brackets, on the right of a type alias and inside
`shape(...)`. Names in bounds, `uses` and brands are syntactically constrained.
Expressions never carry explicit type arguments, so postfix brackets always index.
Constructors, bare variants and qualified functions share expression syntax and are told
apart by name resolution and the expected type. Anonymous and named function-type
parameters are told apart by the colon. A single-variant right-hand side uses `(` where a
type application uses `[`, and a bare name is always an alias. Braces after a condition
begin a block; records are built with parentheses.

A modifier keyword in type position is always followed by a type, and `unique` in
expression position is the prefix extraction `unique x`. Types and expressions never
share a position (`let x: unique T = unique y` switches at `=`). `sendable` is a modifier
exactly when the next token can start a type (a name, `(`, `fn`, `box`, `@` or another
modifier); otherwise it is a type path. After `in` in type position, `{` always opens a
place set, never a block, so a body follows it (`-> T in {x, y} { .. }`). `for` patterns
carry no types, so `for p in e` never meets a relation.
