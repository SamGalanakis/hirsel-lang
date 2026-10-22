# ASCII85 bodies

`std@1/ascii85` is a pure, whole-buffer codec for standalone ASCII85 bodies:

```hirsel
let body = std@1/ascii85.encode(b"\x00\x00\x00\x00Man")
// body == "z9jqo"
let restored = std@1/ascii85.decode(core@1/string.as_bytes(body))
// restored == Ok(b"\x00\x00\x00\x00Man")
```

`encode(data: Bytes) -> unique String` and
`decode(data: Bytes) -> Result[unique Bytes, DecodeError]` borrow their inputs
and return fresh owned results. Decode accepts bytes so ignored control bytes
and invalid non-UTF-8 bytes have unambiguous original byte offsets.

Encoding packs four bytes in big-endian order into five base85 symbols
`!` through `u`. A complete four-byte zero block becomes `z`. A final one,
two or three bytes becomes two, three or four symbols, respectively. Short
all-zero tails use `!!`, `!!!` or `!!!!`, never `z`. Empty input produces an
empty result. There is no line wrapping or padding marker.

Decode follows Go's `encoding/ascii85.Decode` with `flush=true` and sufficient
output space. Every input byte at most ASCII space (0 through 32 inclusive)
is ignored, including NUL, tabs, newlines and controls within quanta or after
a short tail. `z` is legal only between quanta. A final group of two through
four digits is extended with digit 84 (`u`) and emits one fewer byte than its
digit count. A single final digit is an error at EOF.

`Error(CorruptInput(offset))` reports a zero-based byte offset in the original
input, including ignored controls. An invalid symbol or misplaced `z` points
to that byte. A lone final digit points to the original input length,
including trailing whitespace. An error returns no bytes even if earlier
quanta decoded successfully.

The accumulator explicitly wraps modulo 2^32, matching Go's `uint32`
arithmetic for both full and short quanta. In particular, `uuuuu` decodes to
`b"\x08\x78\x0e\xc4"`, and `s8W-\"` decodes to four zero bytes.
These oversized quanta are accepted for compatibility. Decode is **not a
canonicality validator**: controls, `!!!!!` instead of `z`, oversized quanta
and noncanonical short tails can all have other spellings for the same bytes.
Compare re-encoded decoded bytes with the original if canonical spelling is
required.

No `<~` / `~>` delimiters are added or stripped; delimiters are rejected.
There is no `y` shorthand, PDF/PostScript parsing, streaming interface,
charset conversion, RFC1924 alphabet, Z85 alphabet or auto-detection. A byte
outside the ASCII85 alphabet is rejected even if another base85 variant uses
it. Alphabets overlap: Z85's example `HelloWorld` is valid ASCII85 but decodes
to `b"\x7b\xdd\xcb\xd3\xaa\xe7\xf0\xbf"`, not Z85's bytes. Callers must
know which body format they have.

Before reserving storage, encode checks the full-block plus tail capacity
without an overflowing rounding addition. Decode uses checked additions to
count an upper bound: four bytes per `z`, one per other non-control symbol.
Only known ASCII bytes reach String construction. Ordinary runtime allocation
limits still apply.

## Prior art and adaptations

The implementation ports Go `encoding/ascii85/ascii85.go:27-83` (encode) and
`:173-239` (decode and flush) at
`ebe2feefc400f813aefdc8a036e66d2619da6649`. Long-text and zero-block goldens
come from `ascii85_test.go:18-43`; the two original corruption cases come from
`:147-167`. The Go BSD-3-Clause notice is in `third-party/NOTICES.md`.

Adaptations for Hirsel:

- Whole-buffer list builders replace caller-owned slices and partial counts.
  Decode returns an atomic owned Result, without partial output on error.
- A bounded packing loop replaces Go's fallthrough; descending powers of 85
  emit digits forwards instead of filling a five-byte scratch slice backwards.
- Checked capacity arithmetic precedes reservations. A counting pass bounds
  decode storage without multiplying the entire input length by four.
- Explicit wrapping multiplication/addition preserve Go's implicit uint32
  arithmetic under Hirsel's checked ordinary arithmetic.
- Streaming Reader/Writer adapters are outside this API.

Six focused Rust tests run the fixtures in
`crates/hirsel-driver/tests/fixtures/ascii85-9/` on H, LLVM and Cranelift with
validation enabled: 269 fixed assertions and 66 bounded binary roundtrips per
engine. Fixed vectors include lengths 0..9, every byte value, zero tails,
all ignored control bytes, original error offsets, wrapper/variant rejection
and overflowing full/short quanta. Additional expected encodings were produced
with Python's ASCII85 implementation and cross-checked against a local copy
of the pinned Go codec; decoded bytes and offsets were obtained from that Go
copy, independently of the Hirsel implementation.
