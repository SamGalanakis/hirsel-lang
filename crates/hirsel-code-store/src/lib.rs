//! Content-addressed code (RUN-13 code seam): code objects keyed by LANG-6
//! definition hash, a store whose one write is all-or-nothing
//! `add(bundle) -> Ok | Missing(hashes)`, fetch by hash with every object
//! re-hashed on read, an optional signature policy, and a place's code
//! check on arrival.
mod arrival;
pub mod config;
mod object;
mod sign;
mod store;

pub use arrival::{Arrival, CodeIdentity, VersionMismatch, resolve};
pub use object::{CodeObject, ObjectError, hex};
pub use sign::{PublicKey, Signatures, Signer, verify};
pub use store::{Added, Bundle, Closure, CodeStore, StoreError};
