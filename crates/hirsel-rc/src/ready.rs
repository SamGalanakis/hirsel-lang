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
use crate::cleanup::{self, CleanupError, Finalizers};
use crate::legalize::{self, LegalizeError};
use hirsel_lir::ir::LirUnit;
use hirsel_lir::layout_types::LayoutTable;
use hirsel_lir::stage::{Ready, Stage};

#[derive(Clone, Debug)]
pub enum ReadyError {
    Cleanup(CleanupError),
    Legalize(LegalizeError),
}

/// Lower one Rc or Reuse unit to Ready.
pub fn ready<S: Stage>(
    unit: LirUnit<S>,
    table: &LayoutTable,
    finalizers: &Finalizers,
) -> Result<LirUnit<Ready>, ReadyError> {
    let unit = cleanup::lower(unit, table, finalizers).map_err(ReadyError::Cleanup)?;
    legalize::legalize(unit, table).map_err(ReadyError::Legalize)
}
