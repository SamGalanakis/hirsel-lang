//! The existing per-object template closure over compact instance facts.
//! Ready transformations never depend on other Ready bodies merely for LLVM
//! copying: those are Codegen reads, avoiding artificial recursive Ready cycles.
use super::*;
use hirsel_lir::ir::{LirUnit, UnitOrigin};
use serde::{Deserialize, Serialize};

pub type Function = hirsel_rc::arena_credit::equations::Function;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TemplateFacts {
    pub small: bool,
    pub library: bool,
    pub references: Vec<Function>,
}
impl TemplateFacts {
    pub(super) fn of<S>(unit: &LirUnit<S>, finalizers: impl Iterator<Item = Function>) -> Self {
        let mut references = crate::native::units::references(unit)
            .filter_map(|symbol| symbol.instance.map(|id| (symbol.definition, id)))
            .chain(finalizers)
            .collect::<Vec<_>>();
        references.sort();
        references.dedup();
        Self {
            small: unit.instructions.len() <= crate::native::link::SMALL_UNIT_INSTRUCTIONS,
            library: unit.header.origin == UnitOrigin::Library,
            references,
        }
    }
    pub(super) fn canonical(&self) -> bool {
        !self.references.windows(2).any(|pair| pair[0] >= pair[1])
    }
}
