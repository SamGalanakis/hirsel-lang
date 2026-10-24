//! Type-to-layout and ABI bridge; physical schemas and validators live in hirsel-lir.

#[cfg(test)]
hirsel_guard::install!();

pub mod abi;
pub mod instance;
pub mod layout;
pub mod logical_storage;
pub mod operation;
pub mod structural;
pub mod structural_access;
