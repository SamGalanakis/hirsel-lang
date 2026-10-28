//! Independently selected ordinary closure construction, before native import.
//! These records supply C/H inputs; they are not executable custody proofs.
use super::*;
use crate::invocations::ReferenceBody;
use crate::ir::CaptureUse;
use hirsel_syntax::ContentHash;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceClosureContext {
    Scoped,
    OwningScoped,
    Escaping,
    Task,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceClosure {
    pub original: ReferenceBody,
    pub instance: ContentHash,
    pub context: SourceClosureContext,
    pub base_layout: ContentHash,
    pub payload: SourcePayload,
    pub captures: Vec<CaptureUse>,
}

impl SourceClosure {
    pub fn check_structure(&self) -> Result<(), &'static str> {
        if self.original.instance == ContentHash([0; 32])
            || self.instance == ContentHash([0; 32])
            || self.base_layout == ContentHash([0; 32])
            || !matches!(
                self.payload.root_type,
                crate::operation::NativeType::Function(_)
            )
            || !matches!(
                self.payload.recipe.operation,
                PayloadOperation::Allocate { .. }
            )
            || self.captures.len() != self.payload.recipe.inputs.len()
            || self.captures.contains(&CaptureUse::Mut)
            || self
                .payload
                .recipe
                .inputs
                .iter()
                .enumerate()
                .any(|(ordinal, input)| input.capture_slot != Some(ordinal as u32))
        {
            return Err("source closure lacks complete original code, environment or captures");
        }
        Ok(())
    }
}

snapshot_enum!(SourceClosureContext {
    Scoped,
    OwningScoped,
    Escaping,
    Task
});
snapshot_struct!(SourceClosure {
    original,
    instance,
    context,
    base_layout,
    payload,
    captures
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceEnvironments {
    pub entry: Option<SourceClosure>,
    pub constructions: Vec<SourceClosure>,
}
snapshot_struct!(SourceEnvironments {
    entry,
    constructions
});

impl SourceClosure {
    /// Bind complete ordinary capture storage to the independently selected
    /// original environment. No capture is inferred from filtered native order.
    pub fn check_ordinary_entry<S>(
        &self,
        unit: &crate::ir::LirUnit<S>,
        layouts: &crate::LayoutTable,
    ) -> Result<crate::LayoutId, &'static str> {
        self.check_structure()?;
        if self.original.definition != unit.header.definition
            || self.instance != unit.header.instance
        {
            return Err("ordinary capture entry selects another original body");
        }
        if !matches!(
            self.payload.recipe.operation,
            PayloadOperation::Allocate {
                destination: PayloadDestination::Heap
            }
        ) {
            return Err("ordinary receipt captures require their original heap environment");
        }
        let mut candidates = layouts.layouts.iter().enumerate().filter(|(index, base)| {
            base.fields.len() == unit.captures.len()
                && base
                    .fields
                    .iter()
                    .zip(&unit.captures)
                    .all(|(field, value)| {
                        unit.values
                            .get(value.index())
                            .and_then(|value| unit.types.get(value.ty.index()))
                            .is_some_and(|ty| ty.layout == field.layout)
                    })
                && crate::LayoutKeys::rooted(layouts, crate::LayoutId(*index as u32)).0
                    == self.base_layout
        });
        let (index, base) = candidates
            .next()
            .ok_or("ordinary capture entry lacks its source environment layout")?;
        let live = usize::from(matches!(
            base.kind,
            crate::layout_types::LayoutKind::Environment { .. }
        ));
        if base.fields.len() != self.captures.len() + live
            || unit.captures.len() != base.fields.len()
        {
            return Err("ordinary capture entry differs from complete original capture coverage");
        }
        for (ordinal, (&version, field)) in unit.captures.iter().zip(&base.fields).enumerate() {
            let mut slots = unit
                .slots
                .iter()
                .filter(|slot| slot.kind == crate::ir::SlotKind::Capture(ordinal as u32));
            let slot = slots
                .next()
                .ok_or("ordinary capture entry lacks original storage")?;
            if slots.next().is_some()
                || field.ordinal != ordinal as u32
                || unit
                    .values
                    .get(version.index())
                    .is_none_or(|value| value.ty != slot.ty)
                || unit
                    .types
                    .get(slot.ty.index())
                    .is_none_or(|ty| ty.layout != field.layout)
            {
                return Err("ordinary capture entry storage differs from original environment");
            }
        }
        Ok(crate::LayoutId(index as u32))
    }
}
