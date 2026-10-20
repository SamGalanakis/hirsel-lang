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
