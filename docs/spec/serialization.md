# Serialization

Serialization is explicit and separate from storage. A `flat` type is a pointer-free
storage layout, not a wire format; a type is serializable only when it opts in and its
whole schema qualifies. Encoding and decoding follow one canonical, versioned codec that is
independent of the native ABI.

### SER-1: Serializable schemas

Capabilities, resources, handles, functions, raw pointers, owner brands, scoped identity,
opaque types and refinement-constrained types cannot occur in a serializable schema. The
supported schemas are those the canonical codec enumerates: `bool`, the fixed-width
integers and floats, `char`, `String`, `Bytes`, unit, tuples, records, variants, `box`,
`Array`, `List`, `Inline`, `Simd`, `Packed`, `Map`, `Set`, and exactly the serializable
core types below. Every owned payload qualifies independently; no unsupported constructor
inherits a codec.

User records, variants, unconstrained distinct types and flat types may opt in with
`@serializable` when every part of their schema qualifies. Generic schemas require
`serializable` bounds on their serialized parameters. A symbolic schema describes a
generic template and is concretely specialized before encoding or decoding. Encoding
borrows valid data; decoding produces fresh owned data and reconstructs no loans, owner
identities, capability authority or allocation sharing. An opaque or validated domain type
converts explicitly to a serializable data type and validates again on reconstruction.

```hirsel
@serializable
type Point { x: i32, y: i32 }
```

The serializable core types are the errors and outcomes that can cross a call or be
stored, and the data they carry. `Option`, `Result`, `TxFailure` and `Bounded` bound none
of their parameters, so each is serializable exactly when its arguments are, proved after
substitution. `DecodeError`, `CancelCause`, `CancelError`, `ChannelError`, `FileError`,
`FileKind`, `Metadata`, `IpAddr`, `SocketAddr`, `UdpReceived`, `TlsFailure`, `NetError`,
`GrantError`, `ExitStatus`, `ProcessOutput`, `PipelineOutput`, `ProcessError`,
`InputError`, `EnvError`, `OutputError`, `ClockError`, `RandomError`, `WaitError`,
`ArgError`, `FloatIntError`, `OutOfSpace`, `CliError`, `FailureKind`, `SourceSite` and
`Failure` are serializable. Each encodes as its record or variant schema under a nominal
node for package `core`, version `1`. No other core type is serializable: those that hold
a capability (`Deadline`, `Command`), resources, opaque types (`Path`, `Instant`, `Sku`,
and so `DirEntry` and `StockError`), scripted-provider inputs and compile-time shapes
convert through a data type. The exact core identity matters: a same-spelled user type
gets no exemption.

Schema admission and opt-in failures are SER-1 at the declaration or at the encoding use.

### SER-2: Canonical codec

Encoding and decoding are type-directed by an explicit schema and codec version,
independent of native ABI, topology, allocation layout and the run's hash seed. Decoding
validates every tag, length, scalar encoding and data invariant the schema specifies,
rejects noncanonical or trailing input, and returns an expected decode error without
exposing a partially valid value. Neither decoding nor a schema digest establishes
nominal type equality or live authority. Identical canonical values at a fixed schema and
version encode identically. Malformed bytes are an expected decode error, not a
diagnostic.

## Source operations

`codec.encode[T: serializable](value: T) -> Bytes` borrows a valid value and
encodes one version-1 envelope. `codec.decode[T: serializable](data: Bytes) ->
Result[T, DecodeError]` borrows the input and validates it against the concrete
schema of `T` before returning fresh owned data. Both operations require the
whole SER-1 schema, including explicit opt-in for user nominal types.

`DecodeError` has nullary cases: `Version` (magic or codec version),
`SchemaMismatch` (descriptor differs from the expected schema), `Truncated`
(incomplete input), `Trailing` (extra payload or envelope bytes), `Scalar`
(noncanonical bool or NaN, or a `char` that is not a Unicode scalar value),
`Utf8` (invalid String encoding), `Tag` (invalid variant ordinal), `Length`
(invalid or overflowing extent), `Duplicate` (repeated Map key or Set element),
and `Order` (descending Set encodings). Schema or valid source-value
inconsistencies are compiler errors; operational budget exhaustion is separate
from `DecodeError`.

## Canonical data codec version 1

All multibyte integers are fixed-width little-endian. Concatenation has no alignment,
padding, offset table, terminator or implicit length. Lengths and counts are unsigned
`u64`; variant ordinals and descriptor indexes and counts are unsigned `u32`. A byte length
counts octets, and an element count includes zero-byte elements. Values are finite trees:
repeated references encode their data again, and no allocation, pointer, reference count,
sharing or identity number is transmitted. Recursive schemas describe those finite trees
without imposing a language depth limit.

| Schema constructor | Payload bytes, recursively |
|---|---|
| `bool` | One byte, exactly `00` for false or `01` for true. |
| `u8`/`u16`/`u32`/`u64`/`u128`; `i8`/`i16`/`i32`/`i64`/`i128` | Exactly 1/2/4/8/16 bytes; signed integers use two's complement. Encoding rejects an out-of-range supplied scalar rather than truncate it. |
| `char` | Exactly 4 bytes: the scalar value as a `u32`. Decoding rejects a surrogate (0xD800 through 0xDFFF) or a value above 0x10FFFF. |
| `f32`; `f64` | Exactly 4/8 bytes of IEEE binary32/binary64 bits. Every non-NaN bit pattern is preserved, signed zero, subnormals and infinities included. Every NaN encodes as `0x7fc00000` / `0x7ff8000000000000`; decoding rejects every other NaN bit pattern. |
| `String`; `Bytes` | `u64` byte length, then exactly those bytes. A `String` is valid shortest-form UTF-8, with no surrogate encodings and nothing above U+10FFFF; no normalization, source-literal restriction or NUL termination applies. `Bytes` permits any octet. |
| unit; tuple; record | Unit is empty. Tuple elements and record fields are concatenated in declaration order; names and field counts are in the schema only. Variable-size children carry only their own prefixes, never an extra byte extent for the containing field. |
| variant | `u32` zero-based declaration-order arm ordinal, then only that arm's fields in declaration order. No inactive-arm bytes, discriminant compression or niche. |
| `box T`; opted-in transparent nominal or distinct; `Packed[T]` | Only the represented payload's encoding, with no wrapper prefix or allocation marker; the schema keeps each wrapper. `Packed[T]` requires a variant representation and uses that variant's ordinal and active payload, never a native storage maximum. |
| `Array[T]`; `List[T]` | `u64` element count, then exactly that many encoded elements in sequence order. This is an element count even for fixed-width or zero-byte elements, never a byte extent; variable elements are walked sequentially. |
| `Inline[T, n]`; `Simd[T, n]` | Exactly `n` encoded elements in index or lane order, with no count prefix and no tail padding. `Inline` permits `n = 0`; `Simd` requires positive `n` and sized scalar lanes. `n` is bound before a codec runs; symbolic templates are not executable decoders. |
| `Map[K, V]` | `u64` entry count, then key followed by value for each entry in insertion order, preserving the order-sensitive equality of [SEM-5](semantics.md#sem-5-maps). Duplicate keys anywhere in the map are rejected. The key's entire schema satisfies `key`, so it contains no floats; duplicates are found by comparing complete canonical key encodings, nested sets and maps included. |
| `Set[T]` | `u64` element count, then elements sorted strictly increasingly by unsigned lexicographic comparison of their complete canonical encodings; a byte-string prefix precedes its extension. Equal or descending neighbours are rejected. `T` satisfies `key` at every depth. This byte order need not be numeric order or the internal structural order of SEM-5. |
| core `Option[T]`; core `Result[T, E]` | Transparent nominal-plus-variant schemas: `Option` has `None` at ordinal 0 and `Some(T)` at 1; `Result` has `Ok(T)` at 0 and `Error(E)` at 1. Each payload qualifies independently. The exact core identity matters: a same-spelled user type gets no exemption. |

### Concrete schema descriptor

A schema template first substitutes every type and size parameter and proves SER-1. It
rejects missing or out-of-range indexes, unsupported constructors, `Never`, unbound
parameters, malformed builtin arguments, duplicate field or arm names, invalid `Packed` or
`Simd` shapes, and `Map` keys or `Set` elements that are not `key`. Transparent aliases
forward to their target and add no node. The descriptor removes alias forwarders, merges
bisimilar nodes (equal constructor labels, names, exact nominal paths, constant sizes and
ordered child equivalence classes), discards unreachable nodes, and numbers the rooted
graph by declaration-order breadth-first first encounter, root 0. Recursion is a
child-index back-reference. A public descriptor is data, never a proof of source
eligibility.

Descriptor bytes are `u32` descriptor version 1, `u32` root 0, `u32` node count, then the
nodes in that numbering. A text atom is a `u64` UTF-8 byte length followed by the exact
bytes. A child index is a `u32`. Node tags are one byte:

| Tag | Node | Appended |
|---|---|---|
| 0 | Scalar | one byte: `Bool`, `U8`, `U16`, `U32`, `U64`, `U128`, `I8`, `I16`, `I32`, `I64`, `I128`, `F32`, `F64`, `String`, `Bytes`, `Char` as 0 through 15 |
| 1 | Tuple | `u32` child count and child indexes |
| 2 | Record | `u32` field count; per field, text name then child index |
| 3 | Variant | `u32` arm count; per arm, text name, `u32` field count and fields; each field is byte 0 (positional) or byte 1 and a text label, then its child index |
| 4 | Box | child index |
| 5 | Builtin | one byte `Array`, `List`, `Inline`, `Simd`, `Packed`, `Map`, `Set` as 0 through 6; `u32` argument count; each argument is byte 0 and a child index (a type) or byte 1 and a `u64` constant (a size) |
| 6 | Nominal | package-name text, exact version text, `u32` module count and each module text, definition-name text, represented child index |

There are no other node or argument tags. Phantom actuals with identical represented
schemas can have identical bytes; this grants no nominal type equality.

### Envelope and admission

A complete value is the four ASCII bytes `HSC1`, `u32` codec version 1, `u64` descriptor
byte length, the complete descriptor, `u64` payload byte length and the payload. Exactly
one envelope and no trailing bytes are accepted. The decoder receives a concrete expected
checked schema from its caller; it compares descriptor bytes exactly and checks the
version and lengths before constructing data. It never loads a schema, constructor, owner
or capability from incoming bytes. A descriptor digest may index tooling but never
replaces this check or authorizes reconstructing a type or brand. Nested values carry
their payload only, without nested envelopes.

### Validation

Each read and each length addition or multiplication is checked before indexing or
allocating; overflow, truncation, an invalid `bool`, `char`, tag, NaN or UTF-8, duplicate
map keys, noncanonical sets and trailing payload or envelope bytes are rejected. No
partially decoded value is exposed, and no user constructor, callback or finalizer runs.
Operational byte, value or work budgets may refuse service separately from malformed data;
they are not language size or depth bounds. Decoder traversal counts zero-byte elements
and cannot loop forever on an uninhabited recursive schema. For canonical input,
`encode(decode(bytes))` is byte-identical; `decode(encode(value))` preserves the specified
data, with canonical NaNs, canonical `Set` order, fresh ownership and no sharing.

The native in-memory flat layout is private: its byte-extent prefixes, variable-record
framing, SIMD padding and ordinary ABI layout are not this codec. A checked fast path may
copy a payload only with explicit evidence that the formats are equal and with complete
canonical validation. The equal subset is fixed scalars, declaration-order flat tuples and
records, and fixed `Inline` arrays, recursively; no `box`, `Simd`, variant, variable
sequence or ordinary padded layout is assumed equal. Even this subset validates `bool`s
and NaNs before copying, and still writes and checks the envelope. Golden constructor and
malformed-input vectors live in `crates/hirsel-types/tests/wire/`.
