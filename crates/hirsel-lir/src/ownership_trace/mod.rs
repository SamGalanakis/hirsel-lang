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
mod compound;
mod finalizer;
pub mod native;
mod staged;

pub const VERSION: u32 = 7;

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
    /// Internal borrowed-call environment or mutable actual/formal binding.
    /// This creates no holder, transfers no credit and is not a lineage event.
    Alias,
}
snapshot_enum!(Kind {
    Create,
    Move,
    Drop,
    Alias
});

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
    /// The semantic Drop commits only on its live path. Cleanup must relocate
    /// this marker to the actual release/finalizer, never the bypass.
    IfLive,
}
snapshot_enum!(Moment {
    Before,
    After,
    IfLive
});

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
    /// An original return whose owned result is staged across region cleanup.
    /// The original token remains immutable; the bundle checks its exact
    /// store, successful take/return and failure cleanup custody.
    StagedReturn {
        block: BlockId,
        address: AddressId,
    },
    /// An unconditional semantic address drop lowered to adjacent Read Take and
    /// whole Release. The index names Release; the marker retains its original
    /// address identity instead of inventing a creation or move for the load.
    ReleaseDrop {
        block: BlockId,
        index: u32,
    },
    /// One semantic drop whose inline finalizer has mutually exclusive normal,
    /// failed and (for source finalizers) frame-rejection exits. The marker is
    /// committed after FinalizeEnd on the exit actually taken, never at entry.
    /// A Move at this bundle describes the original Drop's failure-edge join;
    /// it commits on body failure/rejected admission only, after cleanup.
    FinalizerDrop {
        block: BlockId,
    },
    /// One semantic aggregate drop, with exact normal/failure cleanup cuts.
    /// Each descendant retirement is an actual finalizer completion; the
    /// aggregate shell never invents a Resource identity or descendant drop.
    CompoundDrop {
        block: BlockId,
        normal: BlockId,
        index: u32,
        failure: BlockId,
    },
}
snapshot_enum!(Point {
    Entry,
    Instruction { block, index, moment },
    Edge(edge),
    CallEntered(block),
    CallReturned(block),
    Return(block),
    StagedReturn { block, address },
    ReleaseDrop { block, index },
    FinalizerDrop { block },
    CompoundDrop { block, normal, index, failure }
});

/// At Create/Drop `token == owner`. At Move `token` is the previous shadow
/// location and `owner` is its recipient. Alias binds a borrowed callable view
/// to its still-live owned caller slot. These are not program-visible values.
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

    /// Committed physical points for one logical marker. Alternatives retain
    /// the same id and cannot authorize a semantic transition. Emitters must
    /// use the checked unit, not retarget a plan to another body.
    pub fn commits<S: Stage>(&self, unit: &LirUnit<S>, id: MarkerId) -> Result<Vec<Point>, Error> {
        if unit.digest() != self.plan.unit {
            return Err(Error::StalePlan);
        }
        let observation = self.observation(id).ok_or(Error::MissingMarker(id))?;
        match observation.point {
            Point::FinalizerDrop { block } if observation.marker.kind == Kind::Move => {
                let Location::Value(parameter) = observation.marker.owner else {
                    return Err(Error::InvalidBinding(id));
                };
                finalizer::failure_edges(unit, block, parameter)
                    .map(|edges| edges.into_iter().map(Point::Edge).collect())
                    .ok_or(Error::InvalidBinding(id))
            }
            Point::FinalizerDrop { block } => {
                let Location::Address(address) = observation.marker.token else {
                    return Err(Error::InvalidBinding(id));
                };
                finalizer::exits(unit, block, address).ok_or(Error::InvalidBinding(id))
            }
            Point::CompoundDrop { .. } => {
                let Location::Address(address) = observation.marker.token else {
                    return Err(Error::InvalidBinding(id));
                };
                let parts = compound::parts(unit, observation.point, address)
                    .ok_or(Error::InvalidBinding(id))?;
                let mut commits = Vec::new();
                for (block, address) in parts {
                    commits.extend(
                        finalizer::exits(unit, block, address).ok_or(Error::InvalidBinding(id))?,
                    );
                }
                Ok(commits)
            }
            Point::StagedReturn { .. } => {
                let staged = staged::binding(unit, observation).ok_or(Error::InvalidBinding(id))?;
                let mut commits = vec![staged.store, staged.take, Point::Return(staged.returning)];
                for (block, _) in unit.blocks.iter().enumerate() {
                    let block = BlockId(block as u32);
                    if finalizer::begin(unit, block)
                        .is_some_and(|(address, _)| address == staged.address)
                    {
                        commits.extend(
                            finalizer::exits(unit, block, staged.address)
                                .ok_or(Error::InvalidBinding(id))?,
                        );
                    }
                }
                Ok(commits)
            }
            Point::ReleaseDrop { block, index } => Ok(vec![Point::Instruction {
                block,
                index,
                moment: Moment::Before,
            }]),
            point => Ok(vec![point]),
        }
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
    InvalidTransport(MarkerId),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ownership trace consistency: {self:?}")
    }
}
impl std::error::Error for Error {}
