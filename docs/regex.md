# Bounded regular expressions

`std@1/regex` ports Go regexp's ordered Thompson/Pike engine at
`ebe2feefc400f813aefdc8a036e66d2619da6649`. Compilation and matching remain
ordinary Hirsel source, with explicit stacks and no host hooks.

At the maximum work budget, searches use a bounded lazy DFA for compiled
programs up to 512 instructions. Admission depends on the pattern and cache
capacity. Subject length never disables it. The cache reuses its allocation on
clearing; after three clears, fewer than ten searched bytes per cached state
triggers scalar continuation from the current ordered frontier. A reverse search
recovers the leftmost start after the forward priority decision, including when
the scalar continuation handles Unicode. Finite work budgets use the ordered
PikeVM to preserve its exact logical charge and `WorkLimit()` behavior.

`compile(pattern, max_instructions)` uses Go's default Perl-style matching
behavior. `compile_flags(pattern, flags, max_instructions)` accepts a `Flags`
record; start with `default_flags()` and set `fold_case`, `multiline`,
`dot_newline`, or `ungreedy`. Inline `(?i)`, `(?m)`, `(?s)`, `(?U)`, combined
flags, `(?-i)`, and scoped forms such as `(?im-s:...)` follow Go's rules.
`\A` and `\z` always anchor the entire original text, including in multiline
mode. Folding and expansion-name properties use Go's pinned Unicode 17 tables;
they do not change std unicode's separate Unicode 16 tables.

`find_captures(re, text, work_limit)` returns `Result[Option[Captures],
MatchError]`. `Captures.spans[0]` is the whole match; subsequent elements follow
opening-parenthesis order, including named `(?P<name>...)` / `(?<name>...)`
groups. Each element is `Option[Span]`: `None()` is an unmatched group, while
`Some(Span(start: n, end: n))` participated with an empty match. Offsets are
UTF-8 bytes relative to the original supplied text. `num_subexp` excludes the
whole match. `subexp_names` includes the empty name at slot zero and empty names
for unnamed groups. Duplicate names are allowed, as in Go.

`find_all_captures(re, text, max_matches, work_limit)` uses the same original
input progression as `find_all`, including suppression of empty matches
immediately following an accepted match. A zero match limit returns immediately.

`replace_all(re, src, replacement, work_limit)` expands `$0`, `$1`, `${name}`
and `$$`. Names are maximal: `$1x` names `1x`; `${1}x` means capture one followed
by `x`. Missing or unmatched groups expand to empty text. Malformed dollar
syntax is retained literally. Duplicate names use the first participating group.
`replace_all_literal` copies replacement text verbatim. `expand(re, mut dst,
template, src, captures, work_limit)` appends to a UTF-8 byte builder. Captures
constructed by callers must have in-range, ordered UTF-8 boundaries for any
referenced span; otherwise it returns `InvalidCaptures()`. An error can leave an
appended prefix in `dst`.

`replace_all_func(re, src, state, callback, work_limit)` calls
`callback(state, src, captures) -> String uses {}` for each replacement. Returned
text is literal. Explicit state and full captures support replacements such as
template variable lookup without hidden closure state. The callback's own
computation belongs to its caller; the regex work budget bounds matching,
capture storage, iteration, template expansion, and bytes copied to the result.

Every call has an independent budget. Reaching `WorkLimit()` leaves the compiled
regex reusable. Capturing instructions count toward `max_instructions`;
noncapturing `(?:...)` groups consume no capture slots. The VM deduplicates by
instruction at each scalar boundary and retains the first thread, so its work
remains bounded by input boundaries, program size and capture count, rather than
by the number of possible paths. Repeated searches share a single operation's
budget, including subject decoding and output copying.

## Differences from Go and their reasons

- Results use `Result`, `Option`, unsigned UTF-8 `Span`s and optional submatches
  instead of nil slices and signed `-1` indices, following Hirsel conventions.
- `Flags` exposes the four user matching flags instead of Go syntax's internal
  bitmask. Parser implementation flags, POSIX mode, literal mode and longest
  matching are outside this existing scalar dialect.
- Strings are valid UTF-8. Go's invalid-byte/byte-interior search behavior cannot
  be represented by Hirsel String. The four RE2 byte-interior `\B` rows are
  skipped by Go's own exec_test.go:153-159 as well.
- Existing unsupported syntax remains unsupported: POSIX/property classes,
  numeric and hexadecimal escapes, quoted `\Q...\E` literals, and byte matching.
  These are separate parser extensions, unrelated to captures/flags/replacement.
  Lookaround and backreferences remain unsupported, as in Go regexp.
- Go has fixed parser count/height limits and several optimized execution paths.
  Hirsel retains checked u64 counts, explicit stacks, caller instruction limits,
  deterministic work limits and an ordered Pike VM for budgeted operations.
  Maximum-budget searches additionally use the lazy DFA described above. Counts beyond Go's fixed 1000 cap may compile if they fit
  the supplied instruction limit; very deep noncapturing groups remain valid.
- SimpleFold uses a materialized sorted scalar-pair table, with Go's ASCII fast
  path, rather than runtime Upper/Lower lookup plus a special orbit table. All
  1,114,112 inputs agree with the pinned Go Unicode 17 implementation. This keeps
  the port independent of std unicode's separately pinned version. Expansion's
  letter/digit categories are also ported from the same Go revision.
- Replacement callbacks receive explicit state, original text and captures
  rather than Go's matched byte/string slice: Hirsel's callback ownership and
  effect signatures make borrowed closure environments explicit. Callback output
  is copied to an owned String builder; callback execution itself is outside the
  regex work budget, as with other caller-supplied algorithms.
- `expand` appends to `mut List[u8]` and returns a result instead of returning a
  byte slice, matching Hirsel's builder convention. Invalid public capture spans
  report `InvalidCaptures()` rather than risking Go's indexing panic. Valid
  capture spans and malformed replacement syntax retain Go behavior.
- Compile errors retain the existing byte-offset `Syntax` / `ProgramTooLarge`
  variants instead of Go's detailed syntax error strings. `MatchError` adds
  `InvalidCaptures()` for safe expansion of externally constructed captures.

The per-row exclusions and upstream test counts are recorded in
[regex-captures-vectors.md](../reports/regex-captures-vectors.md). The older
[regex-vectors.md](../reports/regex-vectors.md) describes the baseline import;
its captures/flags exclusions are historical and superseded here.
