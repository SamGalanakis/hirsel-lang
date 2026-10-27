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
    pub(super) fn check_closed_environments(
        &self,
        request: &Request,
        types: &TypeInterner,
        substitute: &mut impl FnMut(TypeId) -> Result<TypeId, SourceError>,
    ) -> Result<(), SourceError> {
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

    pub(super) fn environment_recipes(
        &self,
        request: &Request,
        types: &TypeInterner,
        parameters: &[u32],
        substitute: impl FnMut(TypeId) -> Result<TypeId, SourceError>,
    ) -> Result<Vec<SourceEnvironment>, SourceError> {
        let anchors = hirsel_tir::source_anchors(&self.definition);
        let payloads = payloads(
            &Context {
                body: self.demand_body(request.ordinary_key())?,
                body_id: hirsel_tir::BodyId(request.ordinary_key().body()),
                types,
                source_anchors: &anchors,
                parameters,
            },
            substitute,
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
