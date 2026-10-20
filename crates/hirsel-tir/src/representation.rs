//! Owner-produced private representations: native layout inputs, never public
//! signature facts. Only the defining owner's query produces one.
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_types::{GenericParameter, Layout, TypeId};

use crate::BodyId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepresentationRecord {
    pub definition: DefPath,
    pub representation_revision: ContentHash,
    pub generics: Vec<GenericParameter>,
    pub private_shape: TypeId,
    pub layout_promises: Layout,
    /// The public resource flag: a resource may have no drop body, so the
    /// finalizer alone cannot imply it.
    pub resource: bool,
    pub finalizer: Option<(DefPath, BodyId)>,
    pub unsafe_ffi_package: bool,
}
