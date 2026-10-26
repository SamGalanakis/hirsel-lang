//! Scope-specific provenance carried by the shared entered-operation proof.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ScopeOpPlan {
    /// The unique installation whose scope SSA value reaches both exits.
    pub enter: CallId,
    pub scope: ValueId,
    /// The proved known callable invocation between installation and exit.
    pub callback: CallId,
}
