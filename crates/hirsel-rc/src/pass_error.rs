//! Failures shared by native transformation passes. Stage attribution belongs
//! to the driver, not to the error's representation.
use crate::instance::InstanceError;
use hirsel_lir::layout::LayoutError;
use hirsel_syntax::DefPath;
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PassError {
    /// A validated shape this pass cannot express yet.
    Unsupported(String),
    /// Input that contradicts the pass's validated stage invariants.
    Malformed(String),
    Instance(InstanceError),
    Layout(DefPath, LayoutError),
}

impl From<InstanceError> for PassError {
    fn from(error: InstanceError) -> Self {
        Self::Instance(error)
    }
}

impl From<hirsel_layout::instance::InstanceBodyError> for PassError {
    fn from(error: hirsel_layout::instance::InstanceBodyError) -> Self {
        Self::Instance(error.into())
    }
}

impl fmt::Display for PassError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
            Self::Malformed(what) => write!(f, "malformed: {what}"),
            Self::Instance(error) => write!(f, "instance: {error:?}"),
            Self::Layout(path, error) => write!(f, "layout of {path}: {error:?}"),
        }
    }
}

impl std::error::Error for PassError {}
