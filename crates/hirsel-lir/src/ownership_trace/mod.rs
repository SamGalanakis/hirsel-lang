//! Optional ownership observations, kept outside ordinary LIR.
//!
//! A plan has no results, effects, control edges or authority. It is not an input
//! to liveness, scheduling, ownership validation or optimization. The ordinary
//! program must pass V/C/H independently; an observation cannot justify a use,
//! release, effect or grant. An emitter may read a checked plan only to emit
//! diagnostic shadow operations at the named committed transitions.
//!
//! Keeping this as a separate artifact avoids changing an untraced unit's
//! snapshot, digest, layout or object key. Nothing constructs or checks a plan
//! on the ordinary compiler path. `Location` names storage, never a resource's
//! identity: engines must carry a fresh dynamic shadow identity through moves,
//! including equal-valued and pointer-free resources and reused storage.
//!
//! Every rewriting pass must supply a relocation recipe, even for an unchanged
//! unit. The checker preserves the immutable marker id, kind and source while
//! checking the relocated operands against the new LIR operation. This checks
//! marker conservation and local correspondence; it does not replace a pass's
//! semantic validation or prove that a producer instrumented every source edge.

use crate::ir::*;
use crate::stage::Stage;
use hirsel_codec::{snapshot_enum, snapshot_struct};
use hirsel_syntax::ContentHash;
use std::collections::BTreeMap;

mod check;
pub use check::{check, check_transform};

pub const VERSION: u32 = 1;

/// Stable within a unit's initial trace plan. Relocation never changes this id.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct MarkerId(pub u32);
impl hirsel_codec::Snapshot for MarkerId {
    fn write(&self, writer: &mut hirsel_codec::Writer) {
        hirsel_codec::Snapshot::write(&self.0, writer);
    }
    fn read(reader: &mut hirsel_codec::Reader<'_>) -> Result<Self, hirsel_codec::DecodeError> {
        Ok(Self(hirsel_codec::Snapshot::read(reader)?))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Kind {
    Create,
    Move,
    Drop,
}
snapshot_enum!(Kind { Create, Move, Drop });

/// A logical shadow location. Field/capture numbers are declaration ordinals,
/// not byte offsets. Addresses retain LIR's checked dynamic projection operands.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Location {
    Value(ValueId),
    Address(AddressId),
    Field { aggregate: ValueId, field: u32 },
    Capture { closure: ValueId, capture: u32 },
    EnvironmentCapture(u32),
    Parameter(u32),
    Return,
}
snapshot_enum!(Location {
    Value(value),
    Address(address),
    Field { aggregate, field },
    Capture { closure, capture },
    EnvironmentCapture(capture),
    Parameter(parameter),
    Return
});

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Moment {
    Before,
    /// Reached only after success, never on the instruction's failure edge.
    After,
}
snapshot_enum!(Moment { Before, After });

/// A commit point, rather than a scheduling point. In particular CallEntered is
/// not the attempted call: pretransfer rejection leaves its actuals with caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Point {
    Entry,
    Instruction {
        block: BlockId,
        index: u32,
        moment: Moment,
    },
    Edge(EdgeId),
    CallEntered(BlockId),
    CallReturned(BlockId),
    Return(BlockId),
}
snapshot_enum!(Point {
    Entry,
    Instruction { block, index, moment },
    Edge(edge),
    CallEntered(block),
    CallReturned(block),
    Return(block)
});

/// At Create/Drop `token == owner`. At Move `token` is the previous shadow
/// location and `owner` is its recipient. These are not program-visible values.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ObserveOwnership {
    pub id: MarkerId,
    pub site: SiteRange,
    pub kind: Kind,
    pub token: Location,
    pub owner: Location,
}
snapshot_struct!(ObserveOwnership {
    id,
    site,
    kind,
    token,
    owner
});

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Observation {
    pub point: Point,
    pub marker: ObserveOwnership,
}
snapshot_struct!(Observation { point, marker });

/// Raw, separately encoded diagnostic input. Decoding grants no authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    pub version: u32,
    pub unit: ContentHash,
    pub observations: Vec<Observation>,
}
snapshot_struct!(Plan {
    version,
    unit,
    observations
});

impl Plan {
    /// Called only by an explicitly enabled trace producer.
    pub fn new<S: Stage>(unit: &LirUnit<S>, observations: Vec<Observation>) -> Self {
        Self {
            version: VERSION,
            unit: unit.digest(),
            observations,
        }
    }
}

/// A pass's explicit per-marker relocation, including a deliberately unchanged
/// placement. Source/id/kind and logical token/owner identities come from the
/// checked input, not this row. Current recipes relocate commit points only;
/// changing an identity requires a separately checked transport rule, which this
/// initial vocabulary deliberately cannot express.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Relocation {
    pub id: MarkerId,
    pub point: Point,
    pub token: Location,
    pub owner: Location,
}
snapshot_struct!(Relocation {
    id,
    point,
    token,
    owner
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recipe {
    pub before: ContentHash,
    pub after: ContentHash,
    pub relocations: Vec<Relocation>,
}
snapshot_struct!(Recipe {
    before,
    after,
    relocations
});

/// Borrowed and checked, so mutation cannot invalidate a check before emission.
/// This wrapper deliberately confers no ordinary LIR-validation authority.
#[derive(Debug)]
pub struct CheckedPlan<'a> {
    plan: &'a Plan,
    markers: BTreeMap<MarkerId, &'a Observation>,
}
impl<'a> CheckedPlan<'a> {
    pub fn plan(&self) -> &'a Plan {
        self.plan
    }
    pub fn observation(&self, id: MarkerId) -> Option<&'a Observation> {
        self.markers.get(&id).copied()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    Version(u32),
    StalePlan,
    MissingRecipe,
    StaleRecipe,
    ChangedUnit,
    DuplicateMarker(MarkerId),
    DuplicateObservation(MarkerId),
    DuplicateRelocation(MarkerId),
    MissingMarker(MarkerId),
    ForgedMarker(MarkerId),
    MissingRelocation(MarkerId),
    ForgedRelocation(MarkerId),
    ChangedMarker(MarkerId),
    IdentityRewrite(MarkerId),
    ChangedOrder,
    InvalidPoint(MarkerId),
    InvalidBinding(MarkerId),
    InvalidSite(MarkerId),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ownership trace consistency: {self:?}")
    }
}
impl std::error::Error for Error {}
