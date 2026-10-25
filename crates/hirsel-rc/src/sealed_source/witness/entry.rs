//! Original callable-entry data, reconstructed without native instances, ABI
//! rows or filtered environment views. This is observation data, not V proof.
use super::*;
use hirsel_core_contract::Passing;
use hirsel_lir::invocations::ReferenceBody;
use hirsel_lir::ir::SourceAnchor;
use hirsel_types::{CaptureContext, FunctionType, Signature, SignatureKind, TypeKind, UseKind};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceParameter {
    pub ordinal: u32,
    pub local: u32,
    pub passing: Passing,
    pub ty: ClosedStorageType,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceCapture {
    pub ordinal: u32,
    pub local: u32,
    /// Original parent place, including its original typed projection.
    pub place: u32,
    pub usage: UseKind,
    pub ty: ClosedStorageType,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceEntryKind {
    Definition {
        declaration: ContentHash,
    },
    Closure {
        producer: SourceAnchor,
        context: CaptureContext,
        callable: ClosedStorageType,
    },
}

/// Body-free original entry account. Original capture ordinals are never
/// filtered by native representation, generic erasure, liveness or layout.
/// Complete selected generic/CT5 coverage is a separate query-owner obligation;
/// this initial closure factory explicitly refuses generic callable binders.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceCallableEntry {
    reference: ReferenceBody,
    source: ContentHash,
    revision: ContentHash,
    kind: SourceEntryKind,
    callable: ClosedStorageType,
    parameters: Vec<SourceParameter>,
    result: ClosedStorageType,
    captures: Vec<SourceCapture>,
    arena_scope_grant: Option<hirsel_tir::ArenaScopeGrant>,
}
impl SourceCallableEntry {
    pub fn reference(&self) -> &ReferenceBody {
        &self.reference
    }
    pub fn source_identity(&self) -> ContentHash {
        self.source
    }
    pub fn implementation_revision(&self) -> ContentHash {
        self.revision
    }
    pub fn kind(&self) -> &SourceEntryKind {
        &self.kind
    }
    /// Full original callable type at this source specialization. It identifies
    /// the entry header, not a selected environment recipe or code authority.
    pub fn callable(&self) -> &ClosedStorageType {
        &self.callable
    }
    pub fn parameters(&self) -> &[SourceParameter] {
        &self.parameters
    }
    pub fn result(&self) -> &ClosedStorageType {
        &self.result
    }
    pub fn captures(&self) -> &[SourceCapture] {
        &self.captures
    }
    /// Original TIR data only: independent V and entry coverage still must
    /// verify the reviewed intrinsic, callback position and actual caller.
    pub fn arena_scope_grant(&self) -> Option<&hirsel_tir::ArenaScopeGrant> {
        self.arena_scope_grant.as_ref()
    }
}

impl SourceInput {
    fn entry(
        &self,
        base: &InstanceKey,
        types: &TypeInterner,
        function: &FunctionType,
        kind: SourceEntryKind,
        substitute: &mut impl FnMut(TypeId) -> Result<TypeId, SourceError>,
    ) -> Result<SourceCallableEntry, SourceError> {
        let body = self.demand_body(base)?;
        if function.foreign.is_some() {
            return Err(SourceError::Unsupported(
                "original foreign entry needs reviewed adapter coverage",
            ));
        }
        if function.incomplete {
            return Err(SourceError::Unsupported(
                "incomplete original callable entry",
            ));
        }
        if body.parameters.len() != function.parameters.len() {
            return Err(SourceError::Malformed("original entry parameter arity"));
        }
        let mut parameters = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for (ordinal, (local, formal)) in body
            .parameters
            .iter()
            .zip(function.parameters.iter())
            .enumerate()
        {
            if !seen.insert(local.0) {
                return Err(SourceError::Malformed(
                    "original entry parameter local repeated",
                ));
            }
            let local_type = body
                .locals
                .get(local.0 as usize)
                .ok_or(SourceError::Malformed("original entry parameter local"))?;
            if local_type.storage != hirsel_tir::Storage::Parameter {
                return Err(SourceError::Malformed("original entry parameter storage"));
            }
            let declared = super::super::closed_storage_type(types, substitute(formal.ty)?)?;
            let actual = super::super::closed_storage_type(types, substitute(local_type.ty)?)?;
            if declared != actual {
                return Err(SourceError::Malformed(
                    "original entry parameter type differs",
                ));
            }
            parameters.push(SourceParameter {
                ordinal: ordinal as u32,
                local: local.0,
                passing: formal.passing.into(),
                ty: actual,
            });
        }
        let result = super::super::closed_storage_type(types, substitute(body.result)?)?;
        if result != super::super::closed_storage_type(types, substitute(function.result)?)? {
            return Err(SourceError::Malformed("original entry result type differs"));
        }
        let callable = match &kind {
            SourceEntryKind::Closure { callable, .. } => callable.clone(),
            SourceEntryKind::Definition { .. } => {
                let mut closed = function.clone();
                for parameter in closed.parameters.iter_mut() {
                    parameter.ty = substitute(parameter.ty)?;
                }
                closed.result = substitute(closed.result)?;
                closed.error = closed.error.map(&mut *substitute).transpose()?;
                let ty = types.intern(hirsel_types::Type::with_kind(TypeKind::Function(closed)));
                super::super::closed_storage_type(types, ty)?
            }
        };
        Ok(SourceCallableEntry {
            reference: ReferenceBody {
                definition: base.definition().clone(),
                instance: base.digest(),
                body: base.body(),
            },
            source: self.source,
            revision: self.revision,
            kind,
            callable,
            parameters,
            result,
            captures: vec![],
            arena_scope_grant: body.arena_scope_grant.clone(),
        })
    }

    /// Use the independently queried original declaration paired with this V
    /// observation. Substitution is the query owner's actual original base
    /// specialization, never a candidate ABI/native environment substitution.
    pub fn named_entry(
        &self,
        base: &InstanceKey,
        types: &TypeInterner,
        signature: &Signature,
        mut substitute: impl FnMut(TypeId) -> Result<TypeId, SourceError>,
    ) -> Result<SourceCallableEntry, SourceError> {
        let function = self.named_declaration(base, signature)?;
        if !function.generics.is_empty() {
            return Err(SourceError::Unsupported(
                "original named entry needs complete binder selection",
            ));
        }
        self.entry(
            base,
            types,
            function,
            SourceEntryKind::Definition {
                declaration: signature.fingerprint,
            },
            &mut substitute,
        )
    }

    fn named_declaration<'a>(
        &self,
        base: &InstanceKey,
        signature: &'a Signature,
    ) -> Result<&'a FunctionType, SourceError> {
        let body = self.demand_body(base)?;
        if signature.definition != self.definition.path
            || signature.fingerprint != self.definition.signature_hash
        {
            return Err(SourceError::Malformed(
                "original entry declaration identity",
            ));
        }
        if base.body() != self.definition.entry.0 {
            return Err(SourceError::Unsupported(
                "named source entry needs its original entry body",
            ));
        }
        if body
            .locals
            .iter()
            .any(|local| local.storage == hirsel_tir::Storage::Capture)
        {
            return Err(SourceError::Malformed(
                "named source entry has closure captures",
            ));
        }
        let SignatureKind::Function(function) = &signature.kind else {
            return Err(SourceError::Malformed(
                "original entry declaration is not callable",
            ));
        };
        Ok(function)
    }

    /// Select a generic named entry using explicit ORIGINAL source actuals.
    /// Rebuild the existing base key before substituting either source binder;
    /// no native view, derived key or callback can stand in for this selection.
    /// Enclosing/CT5 closure binders need a separate complete source account.
    pub fn named_entry_selected(
        &self,
        base: &InstanceKey,
        types: &TypeInterner,
        signature: &Signature,
        actuals: &crate::instance::Actuals,
        memo: &hirsel_types::TypeMemo,
    ) -> Result<SourceCallableEntry, SourceError> {
        let function = self.named_declaration(base, signature)?;
        let body = self.demand_body(base)?;
        if !actuals.outer.is_empty() {
            return Err(SourceError::Unsupported(
                "original named entry needs enclosing binder coverage",
            ));
        }
        if function.generics.len() != actuals.arguments.len() {
            return Err(SourceError::Malformed(
                "original named actual arity differs",
            ));
        }
        if !function.generics.is_empty() && actuals.binder != Some(body.parameter_scope) {
            return Err(SourceError::Malformed(
                "original named binder scope differs",
            ));
        }
        if function
            .generics
            .iter()
            .zip(&actuals.arguments)
            .any(|(formal, actual)| {
                !matches!(
                    (formal, actual),
                    (
                        hirsel_types::GenericParameter::Type { .. },
                        hirsel_types::GenericArgument::Type(_)
                    ) | (
                        hirsel_types::GenericParameter::Size { .. },
                        hirsel_types::GenericArgument::Size(_)
                    )
                )
            })
        {
            return Err(SourceError::Malformed("original named actual kind differs"));
        }
        let expected = crate::instance::key_of(
            types,
            self.definition.path.path(),
            self.definition.entry,
            actuals,
            None,
            None,
        )
        .map_err(|error| SourceError::Type(format!("{error:?}")))?;
        if &expected != base {
            return Err(SourceError::Malformed(
                "original named specialization key differs",
            ));
        }
        let authorities = vec![hirsel_types::AuthoritySummary::Unknown; actuals.arguments.len()];
        let view = hirsel_types::view_digest(types, signature)
            .map_err(|error| SourceError::Type(format!("{error:?}")))?;
        let selected = hirsel_types::instantiate(
            types,
            memo,
            signature,
            view,
            &hirsel_types::Substitution {
                arguments: &actuals.arguments,
                authorities: &authorities,
                roles: &signature.metadata.roles,
                ..Default::default()
            },
        )
        .map_err(|error| SourceError::Type(format!("{error:?}")))?;
        let SignatureKind::Function(mut selected) = selected.kind else {
            unreachable!()
        };
        selected.generics.clear();
        let mut substitute = |ty| {
            crate::instance::substitute(types, memo, ty, actuals, true)
                .map_err(|error| SourceError::Type(format!("{error:?}")))
        };
        self.entry(
            base,
            types,
            &selected,
            SourceEntryKind::Definition {
                declaration: signature.fingerprint,
            },
            &mut substitute,
        )
    }

    /// Select an actual original closure Assign by parent body/block/statement,
    /// then read its checked destination function type AND every original
    /// capture operand. A matching callable signature alone selects no capture.
    /// Generic/CT5 callable binder expansion remains explicitly unavailable;
    /// no filtered native capture view can stand in for that missing proof.
    pub fn closure_entry(
        &self,
        base: &InstanceKey,
        types: &TypeInterner,
        parent: hirsel_tir::BodyId,
        block: hirsel_tir::BlockId,
        statement: u32,
        mut substitute: impl FnMut(TypeId) -> Result<TypeId, SourceError>,
    ) -> Result<SourceCallableEntry, SourceError> {
        let body = self.demand_body(base)?;
        let parent_body = self
            .definition
            .bodies
            .get(parent.0 as usize)
            .ok_or(SourceError::Malformed("original closure parent body"))?;
        let instruction = parent_body
            .blocks
            .get(block.0 as usize)
            .and_then(|block| block.instructions.get(statement as usize))
            .ok_or(SourceError::Malformed("original closure statement"))?;
        let InstructionKind::Assign { destination, value } = instruction.kind else {
            return Err(SourceError::Malformed(
                "original closure producer is not an assignment",
            ));
        };
        let Some(Rvalue::Closure {
            body: target,
            captures,
            context,
            ..
        }) = parent_body.rvalues.get(value.0 as usize)
        else {
            return Err(SourceError::Malformed(
                "original closure producer is not a closure",
            ));
        };
        if target.0 != base.body() {
            return Err(SourceError::Malformed(
                "original closure target body differs",
            ));
        }
        let destination = parent_body
            .places
            .get(destination.0 as usize)
            .ok_or(SourceError::Malformed("original closure destination"))?;
        let original = types
            .get(destination.ty)
            .ok_or(SourceError::Malformed("original closure callable type"))?;
        let TypeKind::Function(original_function) = &original.kind else {
            return Err(SourceError::Unsupported(
                "original closure destination lacks checked callable type",
            ));
        };
        if !original_function.generics.is_empty() {
            return Err(SourceError::Unsupported(
                "original closure entry needs complete CT5 binder selection",
            ));
        }
        let callable = substitute(destination.ty)?;
        let row = types
            .get(callable)
            .ok_or(SourceError::Malformed("original closure callable type"))?;
        let TypeKind::Function(function) = &row.kind else {
            return Err(SourceError::Malformed(
                "original closure destination is not callable",
            ));
        };
        if !function.generics.is_empty() {
            return Err(SourceError::Unsupported(
                "original closure entry needs complete CT5 binder selection",
            ));
        }
        let source_anchors = hirsel_tir::source_anchors(&self.definition);
        let producer = Context {
            body: parent_body,
            body_id: parent,
            types,
            source_anchors: &source_anchors,
            parameters: &[],
        }
        .anchor(block.0 as usize, statement as usize, instruction.span);
        let mut entry = self.entry(
            base,
            types,
            function,
            SourceEntryKind::Closure {
                producer,
                context: *context,
                callable: super::super::closed_storage_type(types, callable)?,
            },
            &mut substitute,
        )?;
        let locals = body
            .locals
            .iter()
            .enumerate()
            .filter(|(_, local)| local.storage == hirsel_tir::Storage::Capture)
            .collect::<Vec<_>>();
        if locals.len() != captures.len() {
            return Err(SourceError::Malformed(
                "original closure capture arity differs",
            ));
        }
        for (ordinal, ((local, declared), operand)) in locals.into_iter().zip(captures).enumerate()
        {
            let source = parent_body
                .places
                .get(operand.place.0 as usize)
                .ok_or(SourceError::Malformed("original closure capture place"))?;
            let ty = super::super::closed_storage_type(types, substitute(source.ty)?)?;
            if ty != super::super::closed_storage_type(types, substitute(declared.ty)?)? {
                return Err(SourceError::Malformed(
                    "original closure capture type differs",
                ));
            }
            entry.captures.push(SourceCapture {
                ordinal: ordinal as u32,
                local: local as u32,
                place: operand.place.0,
                usage: operand.usage,
                ty,
            });
        }
        Ok(entry)
    }
}

#[cfg(test)]
mod tests;
