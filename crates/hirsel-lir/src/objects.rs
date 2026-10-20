//! The native object cache's key and store seam (design §1e, §3).
//!
//! A `UnitKey` hashes everything a unit object's bytes depend on: compiler, LIR,
//! validator, backend and ISA identity, target and opt level, the code-affecting native
//! options, the unit's instance, role, origin and linkage, its TIR digest and
//! implementation revision, the `LayoutKey`s and FnAbi keys it reaches, each callee's
//! symbol and ABI digest, its `CodecKey`s, `debug` and `measurement` (with the resolved
//! source locations under measurement), the manifest rows it uses and the finalizer
//! facts it reads. Units never contain a number the program assigns (`crate::glue`), so
//! one key names the same bytes in every program.
//!
//! An `ObjectStore` maps keys to stored unit fragments. Implementations (`none@1`,
//! `dir@1`, `sim@1`) are selected by the host; none changes program meaning, only what
//! is recompiled.
use hirsel_syntax::ContentHash;
use std::sync::Arc;

/// The content key of one planned unit's object.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct UnitKey(pub ContentHash);

impl UnitKey {
    /// The key's lowercase hex digits, as stores name entries.
    pub fn hex(&self) -> String {
        self.0.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Stored unit fragments by `UnitKey`. A `get` that cannot produce intact bytes is a
/// miss; a `put` that cannot store is dropped. Neither is ever an error the build sees.
pub trait ObjectStore: Send + Sync {
    fn get(&self, key: &UnitKey) -> Option<Arc<[u8]>>;
    fn put(&self, key: &UnitKey, value: &[u8]);
}
