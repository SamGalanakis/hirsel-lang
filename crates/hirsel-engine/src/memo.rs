//! The memo engine's data model: typed reads, revisions and durability
//! (design §1b).
mod reads;
pub use reads::{
    Durability, Inconsistent, LookupKey, ModuleKey, PackageKey, ReadKey, ReadKind, Reads,
    ReadsBuilder, Revision, decode_def_path, encode_def_path,
};
