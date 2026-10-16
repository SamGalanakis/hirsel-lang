# hirsel-span

Own immutable UTF-8 source snapshots and byte locations.

Allowed dependencies: std only.

Invariants and boundary:

- FileId is a u32 index local to one SourceMap; files are immutable snapshots.
- Spans are half-open byte ranges; empty spans support insertions; invalid ranges return None.
- Line and column are one-based; columns count Unicode scalars, not display width (SEM-3).
- LF and CRLF produce one line break; lexer rejects bare CR/invalid UTF-8.
- File paths never determine definition identity (PROG-1/5).

Contract revision 1 retains this representation. Item name spans and tooling-limit
sites use the same byte Span; owning immutable source snapshots remain a driver
responsibility, not a new source/definition identity.
