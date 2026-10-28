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
