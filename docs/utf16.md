# UTF-16 code units

`std@1/utf16.encode(String) -> unique List[u16]` encodes valid Unicode
scalars. `decode_strict(List[u16]) -> Result[String, DecodeError]` returns
`DecodeError(index: u64)` at the first unpaired surrogate, without a partial
String. `decode_lossy(List[u16]) -> String` replaces each unpaired surrogate
with U+FFFD. A valid pair consumes both units; a rejected follower is retried.
For example, D800 D800 DC00 becomes U+FFFD U+10000. An isolated low surrogate
and a trailing high surrogate each produce one replacement.

Indexes count zero-based **u16 code units**, not bytes, Unicode scalars or
graphemes. A supplementary scalar occupies two units. NUL and U+FEFF are
ordinary data: there is no BOM detection, removal or insertion. Inputs are
borrowed; returned lists and strings own independent output storage.

Both conversions take linear time and linear output space. They grow through
existing checked list builders rather than multiplying an input length by a
worst-case expansion factor. All index additions are bounded by the input
length; surrogate composition stays at or below U+10FFFF. Allocation failure
uses the normal language failure mechanism, not DecodeError.

## Explicit byte order

Use `std@1/binary.read_u16` and `append_u16` with a chosen `ByteOrder`.
`ReadError.offset` counts bytes; `DecodeError.index` counts code units.
For a byte stream starting at zero, a unit error is at byte `2 * index`
(use checked arithmetic when translating offsets). Odd byte counts are
truncation errors from `read_u16`, before UTF-16 decoding. The following
example is exercised on the interpreter, H, LLVM and Cranelift:

```hirsel
pub fn main() -> Result[(), ()] {
  let units = std@1/utf16.encode("A\u{10000}\0\u{feff}")
  let orders: List[std@1/binary.ByteOrder] = [LittleEndian(), BigEndian()]
  for order in orders {
    let mut wire: List[u8] = []
    for unit in units {
      std@1/binary.append_u16(unit, order, mut wire)
    }
    let bytes = bytes.from_list(take wire)
    let mut recovered: List[u16] = []
    let mut offset: u64 = 0
    while offset < bytes.len(bytes) {
      match std@1/binary.read_u16(bytes, offset, order) {
        Ok(unit) => list.push(mut recovered, unit),
        Error(_) => failure.check(false, TextInvariant()),
      }
      offset += 2
    }
    failure.check(std@1/utf16.decode_strict(recovered) == Ok("A\u{10000}\0\u{feff}"), TextInvariant())
  }
  Ok(())
}
```

## Prior art and adaptations

Ported from Go `unicode/utf16` at
`ebe2feefc400f813aefdc8a036e66d2619da6649` (BSD-3-Clause; attribution and
license in `third-party/NOTICES.md`): `utf16.go:36-58` surrogate arithmetic,
`:77-112` encoding, and `:125-159` ordered lossy scan. The brief's `:125-144`
range introduces decoding; the actual scan extends through line 159.
`utf16_test.go:51-58` supplies the encoding table and `:133-140` all four
decode rows.

Adaptations: String supplies valid scalars, so Go's invalid rune inputs
(surrogates, negative values and U+110000) cannot be represented and are
excluded; their valid neighbors are retained. Output uses Hirsel's existing
char UTF-8 writer and checked list growth instead of Go rune slices and
allocation sizing. A shared scanner adds strict typed errors beside Go's
lossy policy. No Go allocation-count behavior is promised. Tests add all
surrogate endpoint combinations, independent Unicode scalar answers,
malformed sequences, NUL/BOM, input/output independence and explicit endian
bridging. The 141-scalar sample uses seed 0x16082026 and the Numerical Recipes
LCG, with non-scalars discarded. No runtime or language rule changes.
