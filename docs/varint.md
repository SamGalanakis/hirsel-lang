# Checked base128 integers

`std@1/varint` exports `read_u64(data: Bytes, offset: u64)` and
`read_i64(data: Bytes, offset: u64)`. Both return
`Result[(value, next_offset), varint.ReadError]`. Offsets are absolute in `data`.
`append_u64(value, mut out)` and `append_i64(value, mut out)` append to
`List[u8]`, preserving its prefix and uniqueness; shared snapshots remain intact.

Unsigned base128 writes the least significant seven-bit group first. Bit 7
means another byte follows. This wire order is independent of host endianness
and alignment. Writes use the shortest encoding. Reads accept noncanonical
encodings that fit in ten bytes, including `80 80 80 00` for zero.

Signed values use ZigZag: `0 -> 0`, `-1 -> 1`, `1 -> 2`, `-2 -> 3`.
This is different from signed LEB128 (`-1` is `01` here, not `7f`).
The mapping covers both i64 extrema without negating i64 MIN.

| Error | Absolute offset payload |
| --- | --- |
| `OffsetBeyondEnd(offset)` | Requested cursor exceeds input length |
| `Truncated(offset)` | First missing byte (input length), including an empty read at length |
| `Overflow(offset)` | Invalid tenth byte |

Every read checks its cursor before indexing and advancing. No error returns a
partial scalar or an advanced cursor. At most ten bytes are examined; byte ten
must be `00` or `01`. A tenth continuation byte is **Overflow**, even if the
input ends there. A missing tenth byte after nine continuations is **Truncated**.
An eleventh byte is never examined. Trailing bytes after a successful scalar
remain unread. Callers keep their original cursor on error.

A compact record with unsigned id 300 and signed delta -2 after a marker:

```hir
let mut out: unique List[u8] = [170u8]
varint.append_u64(300, mut out)
varint.append_i64(-2, mut out)
let packet = bytes.from_list(take out)
// Exact bytes: aa ac 02 03
let (id, next) = varint.read_u64(packet, 1)? // (300, 3)
let (delta, end) = varint.read_i64(packet, next)? // (-2, 4)
```

The implementation ports Go `encoding/binary/varint.go:39–132,134–159` at
`ebe2feefc400f813aefdc8a036e66d2619da6649`, with signed extrema/length,
overflow and noncanonical-zero vectors from `varint_test.go:75–227`.
Copyright 2009 The Go Authors; BSD-3-Clause terms are retained in
`third-party/NOTICES.md`. Adaptations: checked absolute cursors, typed errors
without Go's partial accumulator, list-builder appends, and explicit safe
integer conversions. Overflow timing follows **ReadUvarint**, rather than
slice Uvarint's possible eleventh-byte scan. No streaming adapter is exposed.
The independent Rust test oracle generates fixed expected wire bytes using
u128 arithmetic and division, and asserts every truncation of every vector.
