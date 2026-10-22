//! Native holder-scan instruction encoding (runtime descriptor version 15, ABI 37).
//! Eight words per instruction; operands are offsets, counts and linked layout IDs.

pub const WORDS: usize = 8;
pub const HANDLE: u64 = 1;
pub const BLOCK: u64 = 2;
pub const ROOT: u64 = 3;
pub const UNKNOWN: u64 = 4;
pub const INLINE: u64 = 5;
pub const TAG: u64 = 6;
pub const NONNULL: u64 = 7;
pub const LIVE_BIT: u64 = 8;
pub const SEQUENCE: u64 = 9;
pub const REPEAT: u64 = 10;
pub const COLUMN: u64 = 11;

/// Typed handle: offset, family, handle layout, obtainable payload layout (0 absent).
pub const TYPED_HANDLE: u64 = 12;
/// Type-only edge; value scans ignore it.
pub const TYPE_EDGE: u64 = 13;
/// Unavailable static closure information; value scans ignore it.
pub const TYPE_UNKNOWN: u64 = 14;

/// Value-only metadata (runtime ABI 37). Holder scans skip these instructions.
/// VALUE: kind, extent, tag-offset, tag-form, nullary, arm-count, reserved.
pub const VALUE: u64 = 15;
/// VALUE_FIELD: byte-offset, linked-layout, arm (u64::MAX for a record), then zeros.
pub const VALUE_FIELD: u64 = 16;
pub const VALUE_UNKNOWN: u64 = 0;
pub const VALUE_BITS: u64 = 1;
pub const VALUE_BOOL: u64 = 2;
pub const VALUE_FIELDS: u64 = 3;
pub const VALUE_VARIANT: u64 = 4;
pub const VALUE_HANDLE: u64 = 5;

/// Fixed counted pointer value (runtime ABI 40); VALUE operand3 is the linked
/// payload layout. Headers, addresses and reference counts are not values.
pub const VALUE_INDIRECT: u64 = 6;

/// Counted fixed-stride payload (ABI 41). VALUE operands: kind, minimum
/// extent, stride, element-start offset, linked element layout, zero, zero.
pub const VALUE_SEQUENCE: u64 = 7;

/// String/Bytes view (ABI 42). VALUE operands: kind, extent (24), linked
/// byte element layout, then zeros. Owner and start are physical provenance.
pub const VALUE_SLICE: u64 = 8;
