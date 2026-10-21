//! Native holder-scan instruction encoding (runtime descriptor version 14).
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
