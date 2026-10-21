//! Deterministic normalized snapshots of raw native artifacts.
//!
//! The canonical text encoding, `Snapshot` trait, `Writer`/`Reader` and the
//! `snapshot_struct!`/`snapshot_enum!` macros live in `hirsel-codec` (shared
//! with TIR-level code); this module keeps the LIR digest domain. The encoding
//! contains only artifact-local indices and stable identities (DefPaths,
//! content hashes); it never contains a `TypeId`, an address or a
//! host-dependent size. Decoding yields raw artifacts only: a deserialized
//! artifact must be revalidated.
use hirsel_syntax::ContentHash;

/// Shared domain of native snapshot digests. Artifact schemas bind their own
/// versions (e.g. LIR_VERSION); bump this for a change to the shared encoding.
pub const SNAPSHOT_DOMAIN: &str = "hirsel.lir.snapshot.v3";

/// Content digest of the canonical text under a named artifact domain.
pub fn digest<T: hirsel_codec::Snapshot>(domain: &str, value: &T) -> ContentHash {
    hirsel_codec::digest(SNAPSHOT_DOMAIN, domain, value)
}
