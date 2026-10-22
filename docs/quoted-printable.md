# Quoted-printable bodies

`std@1/quoted_printable.encode(Bytes) -> unique String` implements Go Writer's
**binary mode**. CR and LF become `=0D` and `=0A`; there is no newline
normalization. Escapes use uppercase hex. Each line holds at most 75 payload
columns, followed when necessary by `=\r\n` (76 columns before CRLF). An escape
never crosses a line. Only the final space or tab is escaped at EOF, matching
Go's `checkLastByte`: `foo  ` becomes `foo =20`. Interior spaces/tabs, including
ones immediately before a soft break, stay literal. Empty input stays empty.
Every byte, including all 256 possible values, roundtrips exactly.

`decode(Bytes) -> Result[unique Bytes, DecodeError]` implements Go Reader's
compatibility behavior. It is a decoder, not a strict RFC 2045 validator:

- Both `=\r\n` and `=\n` join lines. Spaces/tabs after the equals are accepted as
  transport padding before the line ending.
- Raw CR/LF, tabs and bytes at least `0x80` pass through. Unescaped controls other
  than tab/CR/LF, and DEL (`0x7F`), are errors. No charset conversion occurs.
- Hex escapes accept lowercase. Malformed or short escapes become literal
  equals when the byte following `=` is neither CR nor LF: `=0`, `=G0`, `=0G`,
  and `=x` survive. A valid pair still decodes normally.
- Each physical LF-delimited line loses trailing spaces, tabs and CR/LF; a
  physical LF or CRLF is then restored unless the line ends with soft `=`.
  Consequently `foo\r` decodes as `foo`, while `foo\rbar` preserves its CR.
- The reference's asymmetric EOF rule is deliberate: `foo=` succeeds as `foo`,
  but a lone `=` is an error. `foo= \t` succeeds, while `= \t` fails. `=\r`
  without LF is invalid. Repeated equals can expose a short escape (`abc==`
  produces `UnexpectedEof(3)`).

Errors are atomic: `Error` contains no partially decoded body. All offsets are
zero-based positions in the original encoded input, including across soft joins
and trimmed padding. `InvalidByte(offset)` identifies a raw forbidden byte;
`InvalidHex(offset)` identifies the CR/LF following an equals that cannot be
decoded; `UnexpectedEof(offset)` identifies the incomplete equals escape;
`InvalidSoftBreak(offset)` identifies the terminating equals with an invalid
suffix. As in Go, an error encountered while decoding a line takes precedence
over that line's pending invalid-soft-break error.

The codec handles whole buffers in linear time, with no Go bufio line-size
limit or repeated prefix copying. Encoding checks the exact expanded size and
reserves once before building. Decoding scans physical lines and refers to their
trimmed ranges with virtual preserved newlines; output uses the ordinary checked
list builder. There are no streaming callbacks, text-mode encoder, strict-mode
fork or RFC 2047 header Q API.

## Complete MIME example

Headers are caller-owned and stay outside this body codec. Given these existing
headers (each line ends with CRLF, followed by a blank CRLF line):

```text
MIME-Version: 1.0
Content-Type: application/octet-stream
Content-Transfer-Encoding: quoted-printable

```

The binary body `b"report\x00=\r\n\xff \t"` encodes to
`report=00=3D=0D=0A=FF =09`. Append that ASCII body after the blank line without
encoding the headers. Decoding just that body returns the exact original bytes.
The executable example is
`crates/hirsel-driver/tests/fixtures/quoted-printable-8-mime.hir`.

## Prior art and adaptations

Ported from golang/go `d934b909f6a50247e2a2e9b89de02cdcef93a163`,
`src/mime/quotedprintable/writer.go:31–71,76–168` (binary mode, Close,
write/encode/checkLastByte/soft breaks), `reader.go:30–54,73–140` (hex and line
handling), and all 29 binary writer and 31 reader table vectors in
`writer_test.go:22–118`, `reader_test.go:21–96`. Copyright 2012, 2015 The Go
Authors; BSD-3-Clause, reproduced in `third-party/NOTICES.md`.

Adaptations for Hirsel:

1. Whole-buffer pure builders replace the Go I/O callbacks, pending flush and
   mutable 78-byte line buffer. Token widths account for the final whitespace
   before building; a checked sizing pass reserves the exact output size.
2. Physical-line indices and virtual newline bytes replace bufio/line mutation,
   eliminating its incidental 4096-byte limit. The scan preserves Go's trim,
   literal-equals and error precedence behavior.
3. Typed errors carry original byte offsets; a failed `Result` discards partial
   output instead of exposing Go Reader's successful prefix. I/O errors have no
   counterpart in a pure buffer API.
4. Only binary encoding is exposed. Text-mode vectors are mapped to their Go
   `wantB` output; text-only normalization, callback chunking and I/O failures
   are outside this API.
5. Tests use Hirsel failure assertions and a bounded Rust harness across H,
   LLVM and Cranelift. Go TestExhaustive's four-byte `0A \r\n=` alphabet and
   skip rules retain its independent counts (935 successes, 61 soft-break
   errors, 26 hex errors, 3 EOF errors). Additional independent controls cover
   all bytes, 72–77 columns, literal equals, original offsets, long lines, and
   the complete MIME message above. No corpus step cap is raised.
