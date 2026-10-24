# Markdown rendering and hostile input

The implementation follows commonmark.js at
`898886961b49c1df1175666d03c3ac66c9cc9040` and cmark-gfm at
`27d942c8b0a62d192f616e5bf3578f4b6a89e180`.

The safety policy ports commonmark.js `lib/render/html.js:6-10`
(`potentiallyUnsafe`), `:100-131` (links and images), and `:249-269`
(raw HTML omission). Safe defaults follow cmark-gfm `src/html.c:256-265`
and `:335-353`: raw nodes render `<!-- raw HTML omitted -->`. The opt-in
`Options.allow_unsafe` restores raw HTML and unsafe destinations.
`render_html` always uses safe defaults; `render_html_with_options` accepts
an explicit policy for editable ASTs. GFM enables `Options.tagfilter`,
ported from cmark-gfm `extensions/tagfilter.c:5-60`, independently of unsafe
rendering. It escapes the leading angle bracket of the nine forbidden tags.

`tools/generate-markdown-hostile.py` imports the following pinned vectors,
each as a bounded Hirsel fixture. Programs within one million interpreter
steps run in the conformance corpus. Sixteen measured larger fixtures run in
`crates/hirsel-driver/tests/it-stdlib/stdlib_markdown.rs` with a ten-million-step
budget and the Buck2 runner's timeout. Their sources and expected HTML are
unchanged. This follows the host rule for fixtures above the corpus budget:

| Source | Original vectors | Ported vectors |
| --- | ---: | ---: |
| commonmark.js `test/regression.txt:1-740` | 47 | 47 + 47 malformed twins |
| cmark-gfm `test/regression.txt:1-375` | 26 | 26 + 26 malformed twins |
| commonmark.js `test/test.js:178-300` | 16 families, 31 expanded cases | 31 + 31 malformed twins |
| cmark-gfm `test/pathological_tests.py:14-121` | 21 | 21 + 21 malformed twins |
| commonmark.js `test/test.js:322-370` | 9 safety cases | 9 |
| cmark-gfm `test/spec.txt:9625-9658` | 1 raw HTML example | 1 + 1 malformed twin |

The existing 652 CommonMark examples and four GFM extension sections keep
running through the published-vector harness. This harness explicitly opts
into trusted rendering because the published HTML includes raw nodes.
The separate `check_safe` entry point exercises the default policy.
`SEM-3-markdown-footnotes-run.hir` adds safe-rendering and reference controls.
Seven cmark regression vectors enable its footnote extension. `Options.footnotes`
ports definition containers from `src/blocks.c:1095-1098,1218-1235`, literal
reference recognition from `src/inlines.c:1238-1307`, the collect/resolve/reorder
pass from `src/blocks.c:465-546`, and HTML from `src/html.c:62-101,422-457`.
Footnotes are disabled by default, as upstream; the regression programs enable
exactly the requested extensions. The first cmark regression retains its CR/CR/LF
bytes, as `test/spec_tests.py:96` does.

Additional safety checks cover decoded entities, references, autolinks,
image types, titles, AST rendering, all forbidden tags and unsafe opt-in.

Deviations and reasons:

- Safe rendering is the default, as in cmark-gfm, while commonmark.js makes
  it opt-in. This is the requested untrusted-input policy.
- Unsafe links omit `href` as in commonmark.js; cmark-gfm instead emits
  `href=""`. The requested filter and its test vectors come from commonmark.js.
- Safety and tag filtering live in `Options` and pass through the recursive
  renderer rather than a JavaScript renderer object or C extension callback.
  Hirsel's owned AST and capability-free rendering use explicit values.
  The renderer and footnote pass are separate sources in the same module.
- Sixteen fixtures exceed the corpus's fixed one-million-step budget: cmark
  pathological 1, 6, 9, 14, 16-21; JavaScript pathological 3, 4, 12; cmark
  regressions 8 and 22; JavaScript regression 25. The Rust suite executes these
  same fixtures with ten million steps. All other fixtures use the corpus cap.
- The cmark pathological suite enables only its requested table extension,
  matching `test/pathological_tests.py:141`, rather than all GFM extensions.
- Pathological repetition counts are reduced to fit the corpus interpreter's
  fixed step budget: JavaScript 1000/10000 become 12/24; title backslashes
  10/100/1000 become 10/20/30; cmark repeated patterns become 24, lists 12,
  backtick runs 1..23, and hash collisions 48. All families, size variants,
  exact input patterns and upstream assertions are retained. These bounded
  cases detect functional regressions; they do not establish linear scaling
  at the original upstream sizes.
- cmark's regular-expression expectations are first checked against its
  pinned C renderer, then stored as exact output. This avoids introducing a
  regex dependency into each Hirsel program and strengthens the comparison.
- Malformed twins are additional vectors, independently rendered by the
  pinned implementation. Safety cases use the published safe expectations.
- The GFM checkbox serialization remains the existing spec-compatible HTML
  normalization in `generate-markdown.py`, rather than cmark's equivalent XHTML.

## Fallible output admission (markdown-bounded-output-33)

The shared walk/output structure follows cmark-gfm `src/html.c:468-502` at
`27d942c8b0a62d192f616e5bf3578f4b6a89e180` (BSD-2-Clause; existing
`third-party/NOTICES.md` applies). Whole-file SHA256:
`e2f61240a135e34d595cde1fae9063bbc3ff033720eb52d3acb77fde3cddeb29`.
The paragraph, text and emphasis subtree expectations are copied unchanged from
`api_test/main.c:498-523`, whole-file SHA256
`fd9a392ef45a4285d963c5a1530cafe46107d03c4b5fed363cf17d6d336d953e`.
The bounded fixture also exercises existing safe/raw/GFM serialization and checks
exact-cap equality against unrestricted output. Existing conformance importers
`tools/generate-markdown.py`, `tools/generate-markdown-hostile.py` and
`packages/fixtures/markdown_vectors@1/harness.hir` still call the unrestricted
surface and retain their pinned oracles; no oracle regeneration is required.

The cap and fallible byte sink are Hirsel adaptations, not upstream features.
Every append and escape expansion checks remaining length before emission;
numeric fragments and normalized footnote IDs stream directly instead of
allocating intermediate rendered strings. Borrowed ASCII comparisons preserve
the existing safety policy without allocating a lowercased URL/tag. Bounded
String output reserves its payload cap once rather than using upstream's growing
buffer; allocator bookkeeping/size-class slack and parser/AST memory are separate.
The generic sink owns accepted-prefix cleanup and may refuse late with a typed
error. No Markdown dialect or unrestricted rendering semantics change.

Mdsite's bounded layout streams the five existing `std.html.escape_into`
replacements (Go `src/html/escape.go:166-181`, `ebe2feefc400f813aefdc8a036e66d2619da6649`)
and the existing `std.uri.percent_encode` unreserved alphabet directly to its
admitted buffer. This preserves numeric quote/apostrophe spellings and slash
separators without constructing a temporary navigation or title fragment.

The original full bounded AST and parsed-source aggregate fixtures are retained
in `crates/hirsel-driver/tests/fixtures/markdown-bounded-output-33*.hir`.
Initial interpreter aggregates exceeded the unchanged one-million-step cap.
After literal admission was simplified, the complete AST fixture passes all
four engines under the original interpreter cap. The larger parsed-source
aggregate was moved out of the corpus; its independent full vectors run as Rust
partitions under the same one-million-step cap, preserving every input byte,
expected HTML and assertion. Its original aggregate stop remains recorded.
The small corpus producer is additional coverage, not a shortened original oracle.

The existing safety aggregate passed the original renderer at one million steps.
An initial byte-callback implementation exceeded that same budget; retaining one
admission check for a complete literal span removes redundant per-byte checks
while preserving accounting of every accepted byte and late sink errors. This is
an acceptance fix, not a measured performance claim. The original failed witness
and baseline receipt are retained with the lane's evidence.


## Literal documents and byte scanning

The block/inline split follows the existing commonmark.js and cmark-gfm ports.
For a document with no inline openers or reference definitions, with autolinks
disabled, literal paragraphs, soft breaks, exact thematic breaks and setext
separators can construct the public Node tree directly. This omits the temporary
Entry arena and delimiter bookkeeping. Whitespace-sensitive lines and other
syntax use the general parser. The editable AST and rendered bytes are retained;
`SEM-3-markdown-api-run.hir` covers the literal tree alongside the existing
published CommonMark, GFM and malformed vectors. No parse budget API is added.

Normalization combines NUL replacement, CRLF/bare-CR conversion and line splitting
in one scan, preserving the existing omission of a final empty line. The byte-set
leaf follows Go `src/strings/strings.go:184-240,948-967` at
`dc70946c365d0c23dc2fccc2c3ff94c20c77e3b8`, with a full 256-byte set rather than
an ASCII-only set. It reports offsets relative to the supplied extent and never
retains its inputs. This is shared with the man renderer's ordinary-text scan;
exceptional scalars retain the existing cmark man escaping rules.

## Parse admission (markdown-parse-budget-39)

The existing commonmark.js container (`lib/blocks.js:95-115`) and inline
(`lib/inlines.js:1012-1026`) passes at `8988869` remain the parser. Resource
admission is a Hirsel adaptation, not an upstream feature. The cumulative units
and root-one nesting convention are specified in `docs/spec/std-library.md`.

Admission inventory:

- `new_child` covers block opens, inline text/autolinks, emphasis/link rewrites,
  table rows/cells, email siblings, and footnote backrefs. Finalization and
  unlinking retain their arena charges. Moves reuse entries.
- `reference` admits map entries; `handle_delimiter` admits delimiter records;
  `parse_inlines` admits its sentinel and bracket pushes; `process_emphasis`
  admits its fixed 18 bounds. Failed syntax probes keep their honest charges.
- `table_cells` admits each scratch cell, `table_alignments` each alignment;
  the existing header-line scan admits its line records. Footnotes admit each
  collected definition and used-definition record before insertion/push.
- `normalized_lines` admits line records and NUL's two extra replacement bytes
  before growth. Byte scratch (line assembly, escapes, entities, URI encoding,
  labels and table cells) is derived from admitted source and record extents.
- `build_node` uses an explicit admitted stack, moving completed children into
  parents. Each frame and output Node is charged; nested host recursion is gone.
  Parent-chain and extension traversals use arena links and constant cursor
  storage. `literal_document` admits equivalent logical construction frames and
  Nodes directly. Its old late fallback is decided by `literal_eligible` before
  construction, with no speculative tree or normalization allocated twice.

No renderer limits, language depth policy, V/H rules or runtime ABI change.
Unrestricted parsing disables admission counters in these same passes.
The exact tiny-case and 1x/2x/4x family totals are independently enumerated in
`markdown-parse-budget-39.hir`; existing generated CommonMark/GFM expectations
remain independent unchanged oracles. Original mdman `depth32` bytes now refuse
under root-one32, explicitly qualifying the archived root-zero acceptance.
