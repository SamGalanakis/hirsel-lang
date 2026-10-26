//! Exact source identity for an opt-in entered finalizer. This is only the
//! descriptor/borrow seam: it grants no compact admission, retained address,
//! cleanup coverage or permission to return Pending.
use super::*;
use crate::abi_types::{FnAbiId, ParameterPassing};
use crate::stage::ValidatedUnit;
use hirsel_core_contract::Passing;
use hirsel_syntax::ContentHash;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct FinalizerSource {
    pub layout: LayoutId,
    pub body: ContentHash,
    pub digest: ContentHash,
    pub abi: FnAbiId,
    pub borrowed_self: ValueId,
}

impl FinalizerSource {
    /// Resolve the supplied program association against the actual resource
    /// descriptor and validated Ready body. A matching name alone is insufficient.
    pub fn derive(
        units: &[&ValidatedUnit<Ready>],
        table: &LayoutTable,
        layout: LayoutId,
        body: ContentHash,
    ) -> Result<Self, &'static str> {
        let unit = units
            .iter()
            .find(|u| u.unit().header.instance == body)
            .ok_or("missing finalizer source")?
            .unit();
        let [borrowed_self] = unit.parameters.as_slice() else {
            return Err("finalizer source parameter count");
        };
        let source = Self {
            layout,
            body,
            digest: unit.digest(),
            abi: unit.header.abi,
            borrowed_self: *borrowed_self,
        };
        source.check(units, table)?;
        Ok(source)
    }

    /// Check the supplied association and signature against descriptor and
    /// Ready bytes, independently of any runtime resolver or continuation hash.
    pub fn check(
        &self,
        units: &[&ValidatedUnit<Ready>],
        table: &LayoutTable,
    ) -> Result<(), &'static str> {
        let record = table
            .layout(self.layout)
            .ok_or("unknown finalizer layout")?;
        let declared = record.finalizer.as_ref().ok_or("layout has no finalizer")?;
        let unit = units
            .iter()
            .find(|u| u.unit().header.instance == self.body)
            .ok_or("unavailable finalizer body")?
            .unit();
        if unit.header.definition != declared.definition
            || unit.digest() != self.digest
            || unit.header.abi != self.abi
            || !unit.captures.is_empty()
        {
            return Err("finalizer source identity");
        }
        let abi = table
            .fn_abis
            .get(self.abi.0 as usize)
            .ok_or("missing finalizer ABI")?;
        let [parameter] = abi.parameters.as_slice() else {
            return Err("finalizer ABI parameter count");
        };
        let expected = if unit.header.origin == UnitOrigin::Library {
            (Passing::Mut, ParameterPassing::ExclusiveAddress)
        } else {
            (Passing::Borrow, ParameterPassing::ReadOnlyAddress)
        };
        if !abi.context
            || abi.environment
            || abi.destination
            || abi.diverges
            || abi.result.is_some()
            || abi.result_arena.is_some()
            || parameter.source != 0
            || parameter.layout != self.layout
            || (parameter.convention, parameter.passing) != expected
            || unit.parameters.as_slice() != [self.borrowed_self]
        {
            return Err("finalizer borrowed-self ABI");
        }
        let value = unit
            .values
            .get(self.borrowed_self.index())
            .ok_or("missing finalizer self")?;
        if unit
            .types
            .get(value.ty.index())
            .is_none_or(|ty| ty.layout != self.layout)
        {
            return Err("finalizer self layout");
        }
        Ok(())
    }
}
