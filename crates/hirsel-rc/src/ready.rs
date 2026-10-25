//! The Ready stage: cleanup lowering, then scalar
//! legalization, of one Rc or Reuse unit.
//!
//! A Ready unit has no semantic `Drop`, no `Copy` whose recipe is Bits or
//! FlatBytes, no unresolved call destination, no pending token, no M2
//! placeholder and no operation without a native implementation; LIR has no
//! term for a generic parameter. The Ready validator checks each of these
//! independently of this pipeline.
//!
//! Static loan and origin events (`BeginLoan`, `EndLoan`, `BindOrigin`,
//! `BindFields`, `EndOrigin`) stay in Ready: the Ready validator is what
//! discharges them, by replaying them, so removing them first would remove
//! the evidence it checks. The emitters treat them as no-ops.
use crate::PassError;
use crate::cleanup::{self, Finalizers};
use crate::legalize;
use hirsel_lir::ir::LirUnit;
use hirsel_lir::layout_types::LayoutTable;
use hirsel_lir::stage::{Ready, Stage};

/// Lower one Rc or Reuse unit to Ready.
pub fn ready<S: Stage>(
    unit: LirUnit<S>,
    table: &LayoutTable,
    finalizers: &Finalizers,
) -> Result<LirUnit<Ready>, PassError> {
    ready_indexed(unit, finalizers, &hirsel_lir::LayoutIndex::new(table))
}

/// Ready lowering using the build's immutable common-layout index.
pub fn ready_indexed<S: Stage>(
    unit: LirUnit<S>,
    finalizers: &Finalizers,
    index: &hirsel_lir::LayoutIndex<'_>,
) -> Result<LirUnit<Ready>, PassError> {
    let unit = cleanup::lower_indexed(unit, finalizers, index)?;
    legalize::legalize_indexed(unit, index)
}

/// Opt-in Ready lowering. Both rewriting passes check their own recipes;
/// neither can rebuild an observation from its output or skip a boundary.
/// Ordinary lowering never constructs or checks an ownership plan.
pub fn ready_observed<S: Stage>(
    unit: LirUnit<S>,
    finalizers: &Finalizers,
    index: &hirsel_lir::LayoutIndex<'_>,
    input: &hirsel_lir::ownership_trace::Plan,
) -> Result<
    (
        LirUnit<Ready>,
        hirsel_lir::ownership_trace::Plan,
        Vec<hirsel_lir::ownership_trace::Recipe>,
    ),
    String,
> {
    let (unit, plan, cleanup) = cleanup::lower_observed(unit, finalizers, index, input)?;
    let (unit, plan, legalization) = legalize::legalize_observed(unit, index, &plan)?;
    Ok((unit, plan, vec![cleanup, legalization]))
}
