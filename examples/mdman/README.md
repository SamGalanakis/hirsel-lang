# mdman

A bounded Markdown to man converter using the shipped CommonMark AST. The six
modules are explicit in hirsel.json: CLI/capability projection, manifest policy,
AST validation, roff state/rendering, generic task joins and publication.

From the repository root, copy grants.template.json to a private file beside it,
then run (the template's relative host resolves beside the template):

```sh
hirsel run examples/mdman/hirsel.json --grants examples/mdman/grants.template.json -- check hello.md HELLO 1
hirsel run examples/mdman/hirsel.json --grants examples/mdman/grants.template.json -- render hello.md hello.1 HELLO 1 2026-10-23
hirsel run examples/mdman/hirsel.json --grants examples/mdman/grants.template.json -- render --manifest pages.tsv
```

The fixture directory is the only granted directory, mounted as `data`. For
read-only checks change access to `read`. Stdout and Tasks are explicit grants.
No clock, environment, process or network is needed. For native use, build with
`hirsel build examples/mdman/hirsel.json --backend llvm -o /tmp/mdman`, then run
`/tmp/mdman --hirsel-grants examples/mdman/grants.template.json render hello.md hello.1 HELLO 1`.

Single check syntax is `check INPUT TITLE SECTION [DATE]`; render adds OUTPUT
after INPUT. Manifest syntax is `check|render --manifest FILE`. Each nonempty
manifest line has seven literal tab-separated fields: input, output, title,
section, date, source label, manual. Empty trailing fields are required; no
quoting or escapes. `fixtures/pages.tsv` is an example. Jobs is fixed at four.
All paths are relative ASCII names, at most128 bytes/eight components, with no
`.` or `..` components. Output paths must neither overlap inputs/the manifest,
nor duplicate/contain another output. Create parent directories beforehand.

Limits:16 pages,16KiB source/page,128KiB total source,8KiB manifest,32 AST edges
from document root,8192 AST nodes including document,64KiB output/page. Title,
source label and manual are at most64 UTF-8 bytes, date32, section a digit1–9.
Metadata forbids control characters; empty date remains empty. Source and manifest
reads use a bounded stream with a one-byte overflow probe. Metadata admission
precedes every source open. Output capacity is checked before every append.
The parser allocates its AST before validation: see FRICTION.md for this limitation.

Headings, paragraphs, emphasis, strong, code, links, images, ordered/bullet lists,
quotes, fenced/indented code and thematic breaks follow cmark's no-wrap man
structure. Tables and raw HTML produce source-associated errors. Unsupported AST
variants are rejected exhaustively. AST child paths are zero based; the shipped
AST has no source line spans. Content never enters a roff macro argument; leading
`.` and `'`, backslashes and hyphens are escaped, and metadata quotes use `\(dq`.
Nested font scopes restore their enclosing font. Control bytes become replacement
characters. There is no source-controlled request injection.

All page outputs finish before any publication. Writes occur in destination byte
order, each through an atomic file replacement. A failed file does not erase
preceding successful publications; every failure is reported, followed by counts.
There is no multi-file atomicity or crash durability promise. Check writes nothing.
Domain/resource errors exit2, returned capability errors1, success0. Tooling stops
and language failures keep the launcher's independent exit classes.

Reference: cmark-gfm27d942c8, src/man.c:20–274 and api_test/main.c:569–611;
BSD-2-Clause text in LICENSE.cmark. Deviations: deterministic `.TH` wrapper,
HTML/table rejection, bounded output, source-associated errors, control sanitation,
nested font restoration and deterministic batch publication. No wrapping option.
The focused Rust harness is `man_render_30`; exact engine dispositions and external
receipts are recorded in FRICTION.md. No all-engine claim follows from a static check.

Focused verification (from the repository root):

```sh
python3 tools/buck2/prepare.py
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=man_render_30 --test-arg=--nocapture
```

Native evidence retained on this host uses `-c hirsel.tests=host` and
`--env MAN_RENDER_30_EVIDENCE=/absolute/private/evidence`. Native scripted
file gates/faults remain explicit INCOMPLETE cells; they are never replaced by
claimed cancellation. See FRICTION.md for the original failed/lost receipts and
fixed-bound host replay stops. Successful test wrappers can contain incomplete
tooling cells; inspect the per-cell lines when assessing qualification.
