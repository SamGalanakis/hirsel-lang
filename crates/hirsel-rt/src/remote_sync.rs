//! The atomics the cross-worker heap lists use (`crate::remote`). `proofs/heaps` compiles the
//! same module against its own `remote_sync` with loom's types, so every access in
//! `remote.rs` goes through these names.

pub use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
