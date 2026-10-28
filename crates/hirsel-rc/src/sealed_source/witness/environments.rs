//! Ordinary callback environments are not sealed root payloads. Reconstruct
//! their complete lexical recipes before separating them from that inventory.
use super::*;
use crate::sealed_source::contract::{Port, registry::Request};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceEnvironment {
    entry: SourceCallableEntry,
    payload: SourcePayload,
    profiles: BTreeMap<Port, ClosedStorageProfile>,
}
impl SourceEnvironment {
    pub fn entry(&self) -> &SourceCallableEntry {
        &self.entry
    }
    pub fn payload(&self) -> &SourcePayload {
        &self.payload
    }
    pub fn profiles(&self) -> &BTreeMap<Port, ClosedStorageProfile> {
        &self.profiles
    }
}

impl SourceInput {
    /// Reopen complete original closure entries and profiles. This is source
    /// data validation, not operand-path expansion or executable C/H custody.
    pub fn check_closed_environments(
        &self,
        request: &Request,
        types: &TypeInterner,
        substitute: &mut impl FnMut(TypeId) -> Result<TypeId, SourceError>,
    ) -> Result<(), SourceError> {
        let current = self.demand_projection(request.ordinary_key())?;
        let observed = request.source();
        if current.reference() != observed.reference()
            || current.source_identity() != observed.source_identity()
            || current.implementation_revision() != observed.implementation_revision()
            || current.projection() != observed.projection()
        {
            return Err(SourceError::Malformed(
                "inventory request differs from current original source",
            ));
        }
        if !request
            .source()
            .projection()
            .closures()
            .keys()
            .eq(request.closures().keys())
        {
            return Err(SourceError::Malformed(
                "inventory request lacks complete closure expansion",
            ));
        }
        let parent = self.demand_body(request.ordinary_key())?;
        for (&value, selected) in request.closures() {
            let SourceEntryKind::Closure { producer, .. } = selected.entry().kind() else {
                return Err(SourceError::Malformed(
                    "held source target is not a closure",
                ));
            };
            if producer.body != request.ordinary_key().body() {
                return Err(SourceError::Malformed("held source closure parent differs"));
            }
            if !matches!(parent.blocks.get(producer.block as usize)
                .and_then(|block| block.instructions.get(producer.statement as usize))
                .map(|instruction| &instruction.kind),
                Some(InstructionKind::Assign { value: actual, .. }) if actual.0 == value)
            {
                return Err(SourceError::Malformed(
                    "held source closure selector differs",
                ));
            }
            let current = self.closure_entry(
                selected.ordinary_key(),
                types,
                hirsel_tir::BodyId(producer.body),
                hirsel_tir::BlockId(producer.block),
                producer.statement,
                &mut *substitute,
            )?;
            if &current != selected.entry() {
                return Err(SourceError::Malformed(
                    "held source environment differs from original closure",
                ));
            }
            // The complete original port product must survive body release.
            // This validates its binding and structure, not CT5 completeness.
            let projection = self.demand_projection(selected.ordinary_key())?;
            super::super::contract::registry::Context {
                base: selected.ordinary_key(),
                entry: &current,
                source: &projection,
                ports: selected.profiles(),
            }
            .canonical_bytes()?;
        }
        Ok(())
    }

    /// Reopen the original lexical environments without acquiring this body's
    /// allocation inventory. A selected child can be compiled independently of
    /// its caller, including a caller with ordinary mutation or no receipt ABI.
    pub fn closure_environments(
        &self,
        request: &Request,
        types: &TypeInterner,
        parameters: &[u32],
        mut substitute: impl FnMut(TypeId) -> Result<TypeId, SourceError>,
    ) -> Result<Vec<SourceEnvironment>, SourceError> {
        self.check_closed_environments(request, types, &mut substitute)?;
        self.environment_recipes(request, types, parameters, substitute)
    }

    pub(super) fn environment_recipes(
        &self,
        request: &Request,
        types: &TypeInterner,
        parameters: &[u32],
        substitute: impl FnMut(TypeId) -> Result<TypeId, SourceError>,
    ) -> Result<Vec<SourceEnvironment>, SourceError> {
        let anchors = hirsel_tir::source_anchors(&self.definition);
        let payloads = payloads_selected(
            &Context {
                body: self.demand_body(request.ordinary_key())?,
                body_id: hirsel_tir::BodyId(request.ordinary_key().body()),
                types,
                source_anchors: &anchors,
                parameters,
            },
            substitute,
            |_, rvalue| matches!(rvalue, Rvalue::Closure { .. }),
        )?;
        request
            .closures()
            .values()
            .map(|selected| {
                let entry = selected.entry();
                let SourceEntryKind::Closure { producer, .. } = entry.kind() else {
                    return Err(SourceError::Malformed(
                        "held source target is not a closure",
                    ));
                };
                let mut matching = payloads.iter().filter(|p| &p.recipe.source == producer);
                let payload = matching.next().ok_or(SourceError::Malformed(
                    "held source environment lacks its allocation recipe",
                ))?;
                if matching.next().is_some()
                    || payload.root_type != entry.callable().ty
                    || payload.recipe.inputs.len() != entry.captures().len()
                    || payload
                        .recipe
                        .inputs
                        .iter()
                        .zip(entry.captures())
                        .any(|(input, capture)| {
                            input.capture_slot != Some(capture.ordinal)
                                || input.place != capture.place
                                || input.ty != capture.source_ty.ty
                        })
                {
                    return Err(SourceError::Malformed(
                        "held source environment recipe differs from original captures",
                    ));
                }
                Ok(SourceEnvironment {
                    entry: entry.clone(),
                    payload: payload.clone(),
                    profiles: selected.profiles().clone(),
                })
            })
            .collect()
    }
}

impl SourceInput {
    /// Derive the selected native code and complete ordinary base from reopened
    /// source, before native discovery or candidate units supply either shape.
    pub fn native_environment(
        &self,
        environment: &SourceEnvironment,
        selected: &Request,
        types: &TypeInterner,
        layouts: &mut hirsel_layout::layout::LayoutBuilder<'_>,
    ) -> Result<SourceClosure, SourceError> {
        if selected.declaration().entry() != environment.entry()
            || selected.profiles() != environment.profiles()
            || environment.entry().reference().definition != *self.definition.path.path()
            || environment.entry().source_identity() != self.source
            || environment.entry().implementation_revision() != self.revision
        {
            return Err(SourceError::Malformed(
                "native environment selects another original child",
            ));
        }
        let (native, layout) = crate::instance::receipt_closure_binding(
            types,
            &self.definition,
            environment.entry(),
            layouts,
        )
        .map_err(|error| SourceError::Type(format!("original native environment: {error:?}")))?;
        let SourceEntryKind::Closure { context, .. } = environment.entry().kind() else {
            return Err(SourceError::Malformed(
                "native environment is not an original closure",
            ));
        };
        let context = match context {
            hirsel_types::CaptureContext::Scoped => SourceClosureContext::Scoped,
            hirsel_types::CaptureContext::OwningScoped => SourceClosureContext::OwningScoped,
            hirsel_types::CaptureContext::Escaping => SourceClosureContext::Escaping,
            hirsel_types::CaptureContext::Task => SourceClosureContext::Task,
        };
        let captures = environment
            .entry()
            .captures()
            .iter()
            .map(|capture| match capture.usage {
                hirsel_types::UseKind::Borrow => hirsel_lir::ir::CaptureUse::Borrow,
                hirsel_types::UseKind::Copy => hirsel_lir::ir::CaptureUse::Copy,
                hirsel_types::UseKind::Take => hirsel_lir::ir::CaptureUse::Take,
                hirsel_types::UseKind::Mut => hirsel_lir::ir::CaptureUse::Mut,
            })
            .collect();
        let result = SourceClosure {
            original: environment.entry().reference().clone(),
            instance: selected.aware_key().unwrap_or(&native).digest(),
            context,
            base_layout: hirsel_lir::LayoutKeys::rooted(layouts.table(), layout).0,
            payload: environment.payload().clone(),
            captures,
        };
        result.check_structure().map_err(SourceError::Malformed)?;
        Ok(result)
    }
}
