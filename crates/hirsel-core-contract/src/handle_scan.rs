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

/// Run-context Path (ABI 44); the value is its text, never the intern word.
pub const VALUE_PATH: u64 = 9;

/// Map/Set payload (ABI 46): live entries only, in insertion order. VALUE
/// operands: kind, extent, entries-offset, linked entries-layout, live-offset,
/// linked live-count layout, zero. Hash/index fields are not value metadata.
pub const VALUE_ENTRIES: u64 = 10;

/// ABI 48: closure environment; extent, live-word offset, movable count.
/// VALUE_FIELD operand3 is the live-bit index, or MAX for an always-live field.
pub const VALUE_ENVIRONMENT: u64 = 11;
/// ABI 48: callable pair (code at0, counted environment at8). Operand3=1
/// supplies the concrete closure key in operands4..7 for generic null code.
pub const VALUE_CALLABLE: u64 = 12;
/// ABI 48: iterator triple (environment at0, next at8, optional drop at16).
pub const VALUE_ITERATOR: u64 = 13;

/// ABI 48: declaration-order flat fields, placed after previous active extents.
pub const VALUE_FLAT_FIELDS: u64 = 14;
/// ABI 48: length-prefixed content; operand3=start (8/16), operand4=child.
pub const VALUE_FLAT_SEQUENCE: u64 = 15;
/// ABI 48: Packed active arm; operand3=alignment, operand4=flat, operand6=arms.
pub const VALUE_PACKED: u64 = 16;
/// ABI 48: packed sequence payload; operand3=start, operand4=child.
pub const VALUE_SEQUENTIAL: u64 = 17;
/// ABI 48: SoA payload; operand3=data, operand4=quantum.
pub const VALUE_COLUMNS: u64 = 18;
/// Value-only column: prefix, stride, linked child layout; remaining words zero.
pub const VALUE_COLUMN: u64 = 17;

/// ABI 55: host capability observation only. VALUE operands: kind, extent(8),
/// UTF-8 kind length, then at most 32 bytes of the builtin kind, zero padded.
/// Neither the carrier word nor any capability bytes enter a canonical value.
pub const VALUE_HOST_CAPABILITY: u64 = 19;

/// The core's opaque host handles, excluding inert ticks/paths, scheduler
/// objects, scoped tokens and foreign handles with no host authority identity.
pub fn host_capability_kind(name: &str) -> bool {
    matches!(
        name,
        "Io" | "Args"
            | "Env"
            | "Dir"
            | "Stdin"
            | "Stderr"
            | "Stdout"
            | "Signals"
            | "MonotonicClock"
            | "WallClock"
            | "Random"
            | "Telemetry"
            | "Tasks"
            | "Net"
            | "Proc"
            | "Runner"
            | "File"
            | "AtomicFile"
            | "Mapped"
            | "Watcher"
            | "Span"
            | "Stream"
            | "ReadHalf"
            | "WriteHalf"
            | "Listener"
            | "UdpSocket"
            | "Child"
            | "ChildGroup"
    )
}
