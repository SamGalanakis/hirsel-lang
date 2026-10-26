//! Typed evidence for an entered runtime intrinsic. These records describe
//! retained language storage; runtime invocation custody is the task-owned
//! BoundSite (task generation, bind serial, frame, state and operation kind).
//! Importers reconstruct each supported operation's evidence from Ready and
//! refuse unsupported kinds. An imported helper name never grants authority.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum RuntimeOpKind {
    IsolateBegin,
    ScopeEnter,
    ScopeEnterUntil,
    ScopeExit,
    ScopeExitUntil,
    OrderedJoin,
    OrderedSleepUntil,
    OrderedStdinRead,
    OrderedWatchNext,
    OrderedPrint,
}

impl RuntimeOpKind {
    /// Tags of the shared entered-site binder. Free mutex tails occupy 0..3.
    pub fn tag(self) -> u32 {
        match self {
            Self::IsolateBegin => 24,
            Self::ScopeEnter => 16,
            Self::ScopeEnterUntil => 17,
            Self::ScopeExit => 18,
            Self::ScopeExitUntil => 19,
            Self::OrderedJoin => 32,
            Self::OrderedSleepUntil => 33,
            Self::OrderedStdinRead => 34,
            Self::OrderedWatchNext => 35,
            Self::OrderedPrint => 36,
        }
    }
}

/// The canonical place an entered helper initializes. CallResult is a
/// distinct retained cell, never the emitter's native-stack ABI temporary.
/// ArgumentSlot retains its exact existing mutable loan and reaching None
/// image; it grants no general exclusive-address permission.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum RuntimeResultSource {
    CallResult(ValueId),
    ArgumentSlot {
        parameter: u32,
        address: AddressId,
        slot: SlotId,
        loan: LoanId,
        initial: ValueId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct RuntimeResultSlot {
    pub source: RuntimeResultSource,
    pub layout: LayoutId,
    pub offset: u64,
    pub bytes: u64,
    pub align: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct RuntimeOpPlan {
    pub kind: RuntimeOpKind,
    pub helper: String,
    pub results: Vec<RuntimeResultSlot>,
    /// Source-level scope provenance, separate from the runtime's generation
    /// and current exit phase. Language callback results stay in LiveStorage.
    pub scope: Option<ScopeOpPlan>,
}
