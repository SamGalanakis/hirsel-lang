//! Retained output of the original entered begin. This storage proof grants
//! neither wrapper admission nor permission to leave: callers must separately
//! validate IsolateProtocol and the complete retained callchain/cleanup plan.
use super::*;
use crate::continuation::{
    FRAME_HEADER_BYTES, MAX_FRAME_BYTES, RuntimeOpKind, RuntimeOpPlan, RuntimeResultSlot,
    RuntimeResultSource,
};
use crate::layout_types::{ScalarKind, SizeSpec};
use crate::stage::ValidatedUnit;

impl IsolateProtocol {
    /// Typed handoff to the entered begin runtime. The full protocol, cleanup
    /// coverage and every enclosing frame still require independent checking;
    /// constructing this record does not make an isolate call resumable.
    pub fn begin_operation(
        &self,
        wrapper: &ValidatedUnit<Ready>,
        layouts: &LayoutTable,
        after: u64,
    ) -> Option<RuntimeOpPlan> {
        Some(RuntimeOpPlan {
            kind: RuntimeOpKind::IsolateBegin,
            helper: "hirsel_rt_v2_isolate_begin".into(),
            results: vec![self.begin_result(wrapper, layouts, after)?],
            scope: None,
        })
    }

    /// Check the submitted helper/kind and result against actual Ready SSA.
    /// This neither calls begin_operation nor trusts a runtime/helper digest.
    pub fn check_begin_operation(
        &self,
        wrapper: &ValidatedUnit<Ready>,
        layouts: &LayoutTable,
        after: u64,
        claimed: &RuntimeOpPlan,
    ) -> bool {
        let [result] = claimed.results.as_slice() else {
            return false;
        };
        claimed.kind == RuntimeOpKind::IsolateBegin
            && claimed.helper == "hirsel_rt_v2_isolate_begin"
            && claimed.scope.is_none()
            && self.check_begin_result(wrapper, layouts, after, result)
    }

    /// Append the mark cell after the independently validated live frame area.
    /// The runtime initializes this same output once before its begin yield;
    /// an entered Pending rebinds it rather than materializing a fresh mark.
    pub fn begin_result(
        &self,
        wrapper: &ValidatedUnit<Ready>,
        layouts: &LayoutTable,
        after: u64,
    ) -> Option<RuntimeResultSlot> {
        let unit = wrapper.unit();
        let call = unit.tables.calls.get(self.begin.call.index())?;
        let Callee::Operation(op) = call.callee else {
            return None;
        };
        if !unit
            .tables
            .operations
            .get(op.index())?
            .definition
            .is_core("$intrinsic.IsolateBegin")
            || call.result != Some(self.mark)
            || call.pretransfer_failure.is_some()
            || !matches!(unit.terminators.get(self.begin.block.index()),
                Some(Terminator::Call { call, normal: Some(normal), failure })
                if *call == self.begin.call && Some(*normal) == self.begin.normal && *failure == self.begin.failure)
        {
            return None;
        }
        let value = unit.values.get(self.mark.index())?;
        let ty = unit.types.get(value.ty.index())?;
        let layout = layouts.layout(ty.layout)?;
        let abi = layouts.fn_abis.get(call.abi.0 as usize)?;
        if value.holding != Holding::Plain
            || ty.repr != ValueRepr::Scalar(ScalarKind::U64)
            || layout.kind != LayoutKind::Scalar(ScalarKind::U64)
            || layout.size != SizeSpec::Fixed(8)
            || layout.align != 8
            || abi.result.as_ref()?.layout != ty.layout
            || !abi.result.as_ref()?.borrowed_parameters.is_empty()
            || after < FRAME_HEADER_BYTES
        {
            return None;
        }
        let offset = after.checked_next_multiple_of(8)?;
        if offset.checked_add(8)? > MAX_FRAME_BYTES {
            return None;
        }
        Some(RuntimeResultSlot {
            source: RuntimeResultSource::CallResult(self.mark),
            layout: ty.layout,
            offset,
            bytes: 8,
            align: 8,
        })
    }

    /// Independently check a proposed cell against the actual begin SSA/ABI
    /// and the already checked end of the other frame storage. Does not call
    /// the producer or infer ownership from this numeric offset.
    pub fn check_begin_result(
        &self,
        wrapper: &ValidatedUnit<Ready>,
        layouts: &LayoutTable,
        after: u64,
        claimed: &RuntimeResultSlot,
    ) -> bool {
        let unit = wrapper.unit();
        let Some(Terminator::Call {
            call,
            normal: Some(normal),
            failure,
        }) = unit.terminators.get(self.begin.block.index())
        else {
            return false;
        };
        if *call != self.begin.call
            || Some(*normal) != self.begin.normal
            || *failure != self.begin.failure
        {
            return false;
        }
        let Some(site) = unit.tables.calls.get(call.index()) else {
            return false;
        };
        let Callee::Operation(op) = site.callee else {
            return false;
        };
        if !unit
            .tables
            .operations
            .get(op.index())
            .is_some_and(|o| o.definition.is_core("$intrinsic.IsolateBegin"))
            || site.pretransfer_failure.is_some()
        {
            return false;
        }
        let Some(mark) = site.result else {
            return false;
        };
        let Some(value) = unit.values.get(mark.index()) else {
            return false;
        };
        let Some(ty) = unit.types.get(value.ty.index()) else {
            return false;
        };
        let Some(layout) = layouts.layout(ty.layout) else {
            return false;
        };
        let Some(abi) = layouts.fn_abis.get(site.abi.0 as usize) else {
            return false;
        };
        let Some(result) = &abi.result else {
            return false;
        };
        let Some(offset) = after.checked_next_multiple_of(8) else {
            return false;
        };
        after >= FRAME_HEADER_BYTES
            && offset
                .checked_add(8)
                .is_some_and(|end| end <= MAX_FRAME_BYTES)
            && mark == self.mark
            && value.holding == Holding::Plain
            && ty.repr == ValueRepr::Scalar(ScalarKind::U64)
            && matches!(layout.kind, LayoutKind::Scalar(ScalarKind::U64))
            && layout.size == SizeSpec::Fixed(8)
            && layout.align == 8
            && result.layout == ty.layout
            && result.borrowed_parameters.is_empty()
            && claimed.source == RuntimeResultSource::CallResult(mark)
            && claimed.layout == result.layout
            && claimed.offset == offset
            && claimed.bytes == 8
            && claimed.align == 8
    }
}
