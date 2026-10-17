//! The integration tests of this crate, linked as one binary.
mod abi;
mod arena;
mod box_payload;
mod canonical_wire;
mod closure_loans;
mod defuse;
mod dependencies;
mod flat_sequences;
mod flat_walk_forward;
mod invalid_ir;
mod layout;
mod logical;
mod operation_abi;
mod ownership;
mod packed_storage;
mod projection_proofs;
mod revision10;
mod revision11;
mod revision13;
mod revision26;
mod revision29;
mod revision3;
mod revision4;
mod revision6;
mod revision7;
mod revision8;
mod revision9;
mod schema;
mod shared;
#[path = "../support/mod.rs"]
mod support;
mod tokens;

mod byte_freeze;
