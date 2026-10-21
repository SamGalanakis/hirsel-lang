# Generated body source sites

A generated operation is attributed to its exact generated text, with one-based
lines and Unicode-scalar columns. Its definition is the generated declaration's
DefPath. The source name is `<generated:DefPath>`; it is independent of the host,
declaring file, worker schedule and source-map allocation order. The declaration
is an origin, never a replacement base for the generated offsets. The existing
`GeneratedSource.site` records the `= comptime` origin and is retained in the
validated native source owner. Nested bodies keep their individual TIR spans.

The shared TIR span still names an existing `hirsel_span::Anchor`. The native
boundary numbers the distinct anchors in first-encounter order across the
original definition's bodies: locals, scopes, then each block and its
instructions. The no-source anchor is omitted. LIR `SiteRange.source` selects
that list; `u32::MAX` means no source. Offsets remain relative to that selected
anchor. The list is derived before native rewrites, so optimization cannot
renumber it. Independent native input import verifies every cited anchor and
source snapshot, including generated text, against the engine's current input.
LIR referential validation bounds source selectors and rejects reversed ranges.

This follows rustc's separate stable source-file index and current source-file
map (`compiler/rustc_span/src/source_map.rs:171-202`). Hirsel uses its existing
item/occurrence anchors and a definition-local selector because moved items and
exact replay must retain identity. No second source-map allocator is introduced.

Failure site codes include the selector and both offsets, alongside the
semantic definition and implementation revision. Equal offsets in declaration
and generated sources cannot alias. Heap observations, unit fragments and debug
intervals carry the selector. Cranelift and LLVM encode content-only site indices
during code generation; link-time debug objects resolve them through current
sources. An unmapped source or interval is a gap and cannot inherit a preceding
row. Object reuse therefore does not depend on current file paths or anchor IDs.
Measurement instrumentation, which embeds resolved source data in code, binds
that complete resolved dependency in its emitted artifact identity.

## Compatibility

LIR schema 45 adds `SiteRange.source` and `UnitHeader.source_count`; validator
version 13 checks selectors and range order. Native site
codes use `hirsel.native-site.v2`; fragment magic is `HIRFRG02`. Source-aware debug
rows are part of those fragments. Existing stored objects and import products
are invalidated by the LIR/compiler identity; the old payload is never decoded
as offsets under a declaration anchor. TIR's wire format is unchanged. No
runtime ABI fields are removed, reordered or added by this change.

Heap observation schema 3 retains source selectors. Interpreter observation
schema 3 and driver observation schema 4 include the
resolved source filename. These are output products, never object inputs. The
interpreter execution dependency change advances the conservative semantic
runtime bundle, so recordings from the previous bundle are refused at replay
admission. Within one bundle, interpreter, Cranelift and LLVM recordings retain
the same run identity and source policy. All stored pre-release formats reset
at the 1.0 cut; no automatic tape or cache migration is provided.
