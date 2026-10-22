# Base32

`std@1/base32` encodes owned text and decodes owned bytes:

```hirsel
let identifier = b"\x00\xff\x80\x01\xfe"
let label = std@1/base32.encode(identifier, Hex(), Omitted())
// label == "03VO00FU"
let restored = std@1/base32.decode(label, Hex(), Omitted())
// restored == Ok(identifier)
```

`encode(data: Bytes, alphabet: Alphabet, padding: Padding) -> unique String`
and `decode(input: String, alphabet: Alphabet, padding: Padding) ->
Result[unique Bytes, DecodeError]` borrow their inputs and return owned results.

| Alphabet | Symbols in increasing order |
| --- | --- |
| `Standard()` | `ABCDEFGHIJKLMNOPQRSTUVWXYZ234567` |
| `Hex()` (RFC4648 extended hex) | `0123456789ABCDEFGHIJKLMNOPQRSTUV` |

`Required()` emits `=` padding to a multiple of eight symbols and requires it
when decoding a short final quantum. `Omitted()` emits no padding and rejects
`=` on decode. Empty input and complete eight-symbol quanta need no padding in
either mode. Short unpadded quanta must have 2, 4, 5 or 7 symbols; required
padding is respectively 6, 4, 3 or 1 `=` characters.

Decode ignores CR and LF anywhere, including within and after padding. Spaces,
tabs, lowercase, NUL, non-ASCII bytes and symbols outside the selected alphabet
are invalid. The alphabets overlap: choosing the wrong alphabet may produce
different bytes rather than an error. This is RFC4648 Base32, without Crockford
ambiguity handling or case folding.

As in Go, unused low bits in the last symbol are accepted: `MZ======` and
`MY======` both decode to `b"f"` with the standard alphabet. Decode is therefore
**not a canonicality validator**. Encode always uses the shortest symbol count,
zero unused bits and the selected padding. Re-encode decoded bytes and compare
to the original if canonical spelling is required (CR/LF also affect spelling).

`Error(CorruptInput(offset))` contains a zero-based byte offset into the original
UTF-8 input, including ignored CR/LF. A missing padding suffix points to the
original input length; an unfinished quantum without a padding marker points
to its first symbol. Incorrect padding preserves Go's preceding-byte position,
and invalid padded symbol counts point to the first `=`. Data after completed
padding points to the first trailing non-newline byte. A failing call returns
only the error, even if earlier quanta were valid; it never publishes partial
bytes.

`encoded_len(n: u64, padding: Padding) -> Option[u64]` computes output size
without allocation and returns `None()` if it would overflow `u64`. Encoding
checks this before allocating. Decoding checks a quotient/remainder upper bound
before reserving output. Ordinary runtime allocation limits still apply;
representable sizes do not guarantee available memory.

## Prior art and adaptations

The port follows Go `encoding/base32/base32.go:122-186` (five bytes to eight
symbols and padding tail), `:312-398` (decode quanta and corruption checks), and
`:280-296` (size calculation), at
`ebe2feefc400f813aefdc8a036e66d2619da6649`. Its RFC4648 vectors and all
`TestDecodeCorrupt` rows come from `base32_test.go:21-40,349-393`. Additional
extended-hex and binary goldens were obtained with Go's `encoding/base32`.
The Go BSD license and copyright are recorded in `third-party/NOTICES.md`.

Adaptations:

- A bounded 40-bit `u64` quantum replaces wrapping `u32` halves and fallthrough;
  all shifts are within 0..40, with byte narrowing after masking.
- Quotient/remainder sizing and checked additions precede output reservations.
- Scalar ranges replace the decoder lookup table for the two fixed alphabets.
- A newline-skipping cursor replaces Go's temporary stripped copy so error
  coordinates retain original input offsets.
- Hirsel's owned `Result` is atomic; Go returns partial output on errors.
- Residual counts 1, 3 and 6 in unpadded input are rejected, and data after
  completed padding is rejected. Go's referenced decoder can silently discard
  these inputs; this API enforces complete input consumption.
- No streaming interfaces, custom alphabets, custom padding or assembly paths.

The six bounded Rust tests run 1,653 fixed assertions per engine on H, LLVM and
Cranelift, including every tail, CR/LF at every boundary, original corruption
offsets, all possible unused low bits, and size-overflow boundaries. These
fixtures live under `crates/hirsel-driver/tests/fixtures/base32-8/`.
