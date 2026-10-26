//! Inactive placement proposal for a checked finalizer body. Root custody,
//! cleanup coverage and a typed runtime resolver remain separate obligations.
use super::FinalizerSource;
use crate::continuation::{
    ContinuationPlans, FRAME_HEADER_BYTES, MAX_FRAME_BYTES, NestedFrame, PlanInputs,
    Representation, validate,
};
use crate::ir::SlotId;
use crate::layout_types::LayoutTable;
use crate::stage::{Ready, ValidatedUnit};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct FinalizerFrame {
    pub source: FinalizerSource,
    pub nested: NestedFrame,
    /// The finalizer's immutable self has its declared canonical parameter
    /// record in this frame; the root owner must outlive the entire drive.
    pub self_slot: SlotId,
    pub end: u64,
}

impl FinalizerFrame {
    pub fn propose(
        source: &FinalizerSource,
        units: &[&ValidatedUnit<Ready>],
        inputs: &PlanInputs<'_>,
        plans: &ContinuationPlans,
        after: u64,
    ) -> Result<Self, String> {
        crate::continuation::validate(units, inputs, plans).map_err(|e| e.to_string())?;
        Self::propose_extent(source, units, inputs, plans, after)
    }

    pub(crate) fn propose_extent(
        source: &FinalizerSource,
        units: &[&ValidatedUnit<Ready>],
        inputs: &PlanInputs<'_>,
        plans: &ContinuationPlans,
        after: u64,
    ) -> Result<Self, String> {
        let plan = plans
            .get(source.body)
            .ok_or("finalizer has no compact body")?;
        let parameter = plan
            .parameters
            .first()
            .ok_or("finalizer has no retained self")?;
        let offset = after
            .checked_next_multiple_of(16)
            .ok_or("finalizer extent overflow")?;
        let proposal = Self {
            source: source.clone(),
            nested: NestedFrame {
                body: source.body,
                offset,
                bytes: plan.cost.frame_bytes,
            },
            self_slot: parameter
                .slot
                .ok_or("finalizer self has no canonical place")?,
            end: offset
                .checked_add(plan.cost.frame_bytes)
                .ok_or("finalizer extent overflow")?,
        };
        proposal.check_extent(units, inputs.table, plans, after)?;
        Ok(proposal)
    }

    /// Validate source, complete body continuation metadata, declared self and
    /// placement independently of the producer's offset and bound calculations.
    /// This still grants no runtime admission or permission to return Pending.
    pub fn check(
        &self,
        units: &[&ValidatedUnit<Ready>],
        inputs: &PlanInputs<'_>,
        plans: &ContinuationPlans,
        after: u64,
    ) -> Result<(), String> {
        validate(units, inputs, plans).map_err(|e| e.to_string())?;
        self.check_extent(units, inputs.table, plans, after)
    }

    pub(crate) fn check_extent(
        &self,
        units: &[&ValidatedUnit<Ready>],
        table: &LayoutTable,
        plans: &ContinuationPlans,
        after: u64,
    ) -> Result<(), String> {
        self.source.check(units, table)?;
        let body = plans
            .get(self.source.body)
            .ok_or("missing finalizer plan")?;
        let unit = units
            .iter()
            .find(|u| u.unit().header.instance == self.source.body)
            .ok_or("missing finalizer source")?
            .unit();
        let [parameter] = body.parameters.as_slice() else {
            return Err("finalizer parameter records".into());
        };
        // Library finalizers can mutate self. Their retained exclusive-address
        // proof is not the immutable source-self protocol supplied here.
        if unit.header.origin != crate::ir::UnitOrigin::Source
            || body.representation != Representation::Resumable
            || body.digest != self.source.digest
            || parameter.value != self.source.borrowed_self
            || parameter.source != 0
            || parameter.slot != Some(self.self_slot)
            || parameter.layout != self.source.layout
            || !unit
                .slots
                .get(self.self_slot.index())
                .is_some_and(|s| s.kind == crate::ir::SlotKind::Parameter(0))
            || self.nested.body != self.source.body
            || self.nested.bytes != body.cost.frame_bytes
        {
            return Err("finalizer retained body/self".into());
        }
        if after < FRAME_HEADER_BYTES
            || self.nested.offset < after
            || !self.nested.offset.is_multiple_of(16)
            || self.nested.offset - after >= 16
            || self.nested.bytes < FRAME_HEADER_BYTES
            || self.nested.offset.checked_add(self.nested.bytes) != Some(self.end)
            || self.end > MAX_FRAME_BYTES
        {
            return Err("finalizer retained extent".into());
        }
        Ok(())
    }
}
