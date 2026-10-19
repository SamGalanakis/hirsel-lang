//! A place's code check on arrival (RUN-8 step 3): run the request's code
//! only when the place holds it, from its own revision or its code store.
use crate::object::CodeObject;
use crate::store::{Closure, CodeStore, StoreError};
use hirsel_syntax::{ContentHash, DefPath};
use std::collections::{BTreeMap, BTreeSet};

/// The code a request names: a definition path and its definition hash.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodeIdentity {
    pub path: DefPath,
    pub hash: ContentHash,
}

/// Where a place found a request's code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Arrival {
    /// The place's own revision defines the path with this hash.
    Local,
    /// The code store holds the hash and its whole closure, verified.
    Fetched(BTreeMap<ContentHash, CodeObject>),
}

/// The place cannot run the request's code: `NotRun(VersionMismatch())`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionMismatch {
    /// The hashes of the closure the place lacks.
    pub missing: BTreeSet<ContentHash>,
    /// The store failure that kept the place from finding the code, if any.
    pub cause: Option<StoreError>,
}

/// Resolve `identity` at a place whose revision defines `local` (each path
/// with its definition hash) and which fetches through `store`. Code is
/// never fetched when the revision already has it, and fetched code is
/// verified object by object; a store failure is a version mismatch.
pub fn resolve(
    identity: &CodeIdentity,
    local: &BTreeMap<DefPath, ContentHash>,
    store: &CodeStore,
) -> Result<Arrival, VersionMismatch> {
    if local.get(&identity.path) == Some(&identity.hash) {
        return Ok(Arrival::Local);
    }
    match store.closure(&identity.hash) {
        Ok(Closure::Complete(objects)) => Ok(Arrival::Fetched(objects)),
        Ok(Closure::Missing(missing)) => Err(VersionMismatch {
            missing,
            cause: None,
        }),
        Err(cause) => Err(VersionMismatch {
            missing: BTreeSet::from([identity.hash]),
            cause: Some(cause),
        }),
    }
}
