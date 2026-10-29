# hirsel-span

Own immutable UTF-8 source snapshots and byte locations.

Allowed dependencies: std, serde, and hirsel-intern (for id-salt scrambling of anchor ids).

Invariants and boundary:

- FileId is a u32 index local to one SourceMap; files are immutable snapshots.
- Spans are half-open byte ranges; empty spans support insertions; invalid ranges return None.
- Line and column are one-based; columns count Unicode scalars, not display width (SEM-3).
- LF and CRLF produce one line break; lexer rejects bare CR/invalid UTF-8.
- File paths never determine definition identity (PROG-1/5).

Contract revision 1 retains this representation. Item name spans and tooling-limit
sites use the same byte Span; owning immutable source snapshots remain a driver
responsibility, not a new source/definition identity.

A `Span` is a byte range relative to an `Anchor`: each file has one with base 0,
and each top-level item occurrence has one at the start of its owned tokens.
`SourceMap::move_anchor` repositions an item without touching the spans inside
it. `resolve` gives the `FileSpan` (file and file offsets), which only rendering
and output use.

Anchors are process-unique (`Anchor::fresh`, scrambled under `id-salt`), so a
span from one session never resolves by accident in another. Serialized spans go
through an `AnchorCodec` (`with_anchor_codec`): the encoder numbers the anchors it
meets from 1 and the decoder maps those numbers to anchors the receiver allocated
and placed; 0 is no anchor. Package images and precheck records carry each
anchor's (source ordinal, base) beside the numbered spans, so the receiver places
them without rewriting any span.

`AnchorCodec::Raw` preserves numeric positions while an independently framed
archive validates its scalar rows. It masks an enclosing codec; typed consumption
still binds those positions to the reader's current source owners.
