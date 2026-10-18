//! The integration tests of this crate, linked as one binary.
mod arena;
mod box_payload;
mod capture_mask;
mod clock;
mod defuse;
mod entry_return;
#[path = "../fixtures/mod.rs"]
mod fixtures;
mod heap;
mod moved_variant;
mod nested_regions;
mod nested_views;
mod niche;
mod operations;
mod programs;
mod revision3;
mod revision4;
mod revision7;
mod vectors;
mod versions;
