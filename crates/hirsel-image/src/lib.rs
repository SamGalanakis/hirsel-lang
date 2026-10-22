//! The std image's file format (design §1(f)): one mmapped file holding a
//! package's checked products, so a program reads them without parsing or
//! checking the package. This crate owns the container; `hirsel_engine::image`
//! decides what its sections hold.
//!
//! Every session id in the file is a number in a table the file carries:
//! symbols and definition-path components in the string table, `DefId`s in the
//! definition table, `TypeId`s as positions in the one canonical type table,
//! and span anchors in the anchor table. The writer numbers each table in the
//! order a deterministic encoding first meets an id (sections in kind order,
//! keyed entries in DefPath order), so the bytes depend only on the products,
//! never on id assignment, interning order or `--jobs`.
//!
//! The type table decodes eagerly into an immutable `TypeBase`: position p is
//! base row p in every session interner built over the image's base, so a
//! decoded value's `TypeId`s are valid in all of them and a decoded entry can
//! be shared by every session. Entries decode lazily, one at a time.
//!
//! Layout (integers little-endian):
//!
//! | offset | field |
//! |---|---|
//! | 0 | magic `HIRSIMG1` |
//! | 8 | format version (u32) |
//! | 12 | section count (u32) |
//! | 16 | key (32 bytes) |
//! | 48 | directory: per section kind (u32), count (u32), offset (u64), length (u64) |
//!
//! Sections, in directory order:
//! - strings: count+1 u32 offsets, then the UTF-8 bytes;
//! - definitions: count+1 u32 offsets into a u32 word area, then the words;
//!   definition n's words are its package name, package version, item name
//!   and modules, as string numbers;
//! - definition index: count (a power of two) u32 slots, each a definition
//!   number plus one or 0, probed linearly from the low bits of the
//!   definition's DefPath fingerprint;
//! - anchors: per anchor number from 1, the source's manifest ordinal and the
//!   anchor's base in it (u32 each);
//! - types: the canonical `TypeTable` (bincode);
//! - then the user sections in kind order (kinds from `FIRST_KIND`): a blob
//!   is one bincode value; a keyed section is count entries of definition
//!   number, start and end (u32 each, offsets in the section) in definition
//!   order, then the values.
#[cfg(test)]
hirsel_guard::install!();

mod map;
mod read;
mod write;

pub use read::Image;
pub use write::{Encode, ImageWriter, Written, encode_value, write_atomically};

/// The first byte of every image.
pub const MAGIC: [u8; 8] = *b"HIRSIMG1";
/// The container's version; the key binds the products' own versions.
pub const VERSION: u32 = 3;
/// The first kind a user section may have.
pub const FIRST_KIND: u32 = 16;

const HEADER: usize = 48;
const DIRECTORY_ENTRY: usize = 24;
const STRINGS: u32 = 1;
const DEFS: u32 = 2;
const DEF_INDEX: u32 = 3;
const ANCHORS: u32 = 4;
const TYPES: u32 = 5;
const KEYED_ENTRY: usize = 12;

/// The bincode configuration of every value in an image.
fn options() -> impl bincode::Options {
    bincode::DefaultOptions::new()
}

/// The slot `fingerprint` starts probing at, in a table of `slots` (a power of two).
fn first_slot(fingerprint: hirsel_syntax::ContentHash, slots: usize) -> usize {
    let low = u64::from_le_bytes(fingerprint.0[..8].try_into().expect("8 bytes"));
    (low as usize) & (slots - 1)
}

#[cfg(test)]
mod tests;
