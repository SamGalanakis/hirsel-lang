//! The memo engine's data model: typed reads, revisions and durability
//! (design §1b).
mod reads;
pub use reads::{
    Durability, Inconsistent, LookupKey, LookupSource, ModuleKey, PackageKey, ReadKey, ReadKind,
    Reads, ReadsBuilder, Revision, decode_def_path, encode_def_path,
};
mod graph;
pub use graph::{
    ChangeSet, Invalidation, Memo, MemoGraph, MemoKey, MemoStats, Verdict, invalidation_count,
};
mod depth;
pub use depth::{DepthEvidence, SiteSpan, dependency_limit};
mod coordinator;
pub use coordinator::{CoordinatorStats, Entered, Frame, S1Coordinator, S1Worker, WorkerId};
mod holds;
pub use holds::{HoldStats, Holds, Retention, default_hold_bytes};
mod retained;
pub(crate) use retained::retained_tir_bytes;
mod worklist;
pub use worklist::{Step, WorkKey, WorkKind, Worklist};
