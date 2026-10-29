# Immutable product records

Ordinary records retain lexical source owners, canonical type references and a
semantic payload. `StoredRecord` borrows its payload and retains the complete
immutable backing and attached admission owner. Opening an envelope does not
validate its semantic payload or supply verifier authority.

Type component domain `2/2` uses little-endian integers and indexed rows:

| Region | Encoding |
| --- | --- |
| Header, 32 bytes | `HIRTY002`, member count u32, dependency count u32, cyclic flag u32, zero u32, complete length u64 |
| Dependencies | First-use ordered distinct `{Output[32], length u64, member u32}` rows |
| Directory | One 32-byte descriptor per member: label offset u64, label length u64, edge offset u64, edge count u32, type tag u32 |
| Member data | Scalar label, zero padding to a four-byte boundary, then u32 edge positions |

Labels use the existing fixed-integer type schema with all type edges replaced
by the dangling placeholder. The edge array carries those edges in semantic
order. Positions below the member count name local rows; later positions name
dependencies. Type tags follow TypeKind order and are checked against each label.
Each label/edge array starts exactly where its predecessor ends, with only the
specified zero padding. There are no archive pointers or Rust layout casts.

`CheckedTypeComponent::open` checks the expected framed Hirsel content reference
and validates framing, checked arithmetic, row bounds, tags, labels, padding,
member indices, dependency order and the complete SCC before exposing row views.
Canonical member numbering reuses Hirsel's SCC keys and ordered DFS, with exact
rooted encodings deciding key ties. Each external child requires its own checked
open. An archive's SHA and structural checks grant no V acceptance.

`TypeRow` reads a checked kind/edge without constructing a TypeTable or allocating
TypeIds. `materialize` creates one row when a transformation or interner needs
ownership. The existing shared TypeBase resolves positions lazily through these
same checked rows. Schema 1's whole-table component decoder has no fallback.

Validation follows rkyv d963abc24301a571605a9bafd963dfed72187fd7,
`rkyv/src/validation/archive/validator.rs:135-157`, and Cap'n Proto
b5ee48b9f3586fc83af8c429623e2c0d2435ec59,
`c++/src/capnp/layout.c++:3024-3055`. Hirsel uses bounded byte arrays and explicit
member indices instead of relative Rust pointers or pointer-tree canonicality.
The caller admits backing and validation scratch. `Bytes::visit_retained_allocations`
reports the full backing through subslices to the existing shared-charge policy.
Process allocation keys never enter portable records.

`IndexedEnvelope` is the ordinary record layout emitted by
`TypeRecords::encode_record`. The former bincode Record reader/writer is removed,
with no schema-1 fallback. Its 48-byte header contains
`HIRREC02`, total length u64, anchor/type counts u32, type-array offset u64,
payload offset u64 and payload length u64. It is followed by 16-byte anchor
extent descriptors, 44-byte type references, contiguous source-owner rows and
the payload. Source rows use u32 lengths/counts, UTF-8 strings and a u32
occurrence. Integer reads copy fixed-endian bytes and require no Rust alignment.

Opening checks an exact expected Reference and validates all framing, extents,
counts, UTF-8 and source-row consumption without allocating paths or a type
reference array. The owner retains that Reference and its complete backing.
Source strings and module iterators borrow the checked bytes. Payload subslices
retain the same backing and admission. Each demanded type component checks its
own bytes independently. A checked envelope does not validate the opaque inner
payload or provide V authority; Lower/native schemas must supply their readers.

Type labels always carry complete definition paths and symbol spellings. Opening
a lazy component inside V's name numbering or an image decoding scope masks those
enclosing tables for label validation and materialization, then restores them.
Child validation cannot add names to a parent identity or interpret inline names
as the parent's numeric slots. The same scope governs SCC labels and base labels.

`CheckedTir` opens source TIR in domain `0x4e000024/2`. Its envelope payload is
an indexed tree of row arrays. Every array starts with `HIRTIR01`, a u32 schema
tag, a u32 count and a u64 total length, then one `{offset u64, length u64}`
descriptor per row. Rows are contiguous in declaration order. Definition, body,
block and match headers name bounded subarrays; instructions, locals, places,
rvalues, cleanups, types, parameters and match arms are independently readable.
The scalar definition header also binds the current `TIR_VERSION`.

Opening checks all array extents/tags/counts, exact scalar encodings, source and
type position bounds, distinct portable references and semantic first-use order.
Scalar checks mask enclosing source/name/type codecs. Each demanded type resolves
through its independently checked component. `TirView` presents the same field
contract over a live Definition and an archive; archived access decodes one row,
without constructing a Body or TypeTable. `materialize` is an explicit transform
boundary. `NativeRecords::open_tir` retains current source bindings and the caller's
admission owner on the complete backing. Neither view supplies V acceptance.

Ordinary publication and indexed TIR share one portable position builder. It
merges equal type references and equal lexical source owners in semantic first-use
order, then writes payload positions under that mapping. Interner/anchor sharing
and previous root requests cannot change those positions. `StoredRecord` retains
the checked backing and reads its source/type directories directly; it retains no
copied source-path or type-reference arrays. Raw byte-slice transformation callers
use transient reference/anchor handles under the same framing and child checks.
