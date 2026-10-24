//! Ordinary syntax archives for a demanded file's written sibling headers.
//! Sibling lookup occurs inside the admitted front poll and performs only
//! validated transport; no source parser or compiler phase is called here.
use super::*;
pub(super) struct SiblingArchives {
    pub files: BTreeMap<FileId, Vec<Reference>>,
    pub sources: SourceMap,
    pub owners: Arc<crate::NativeRecordContext>,
    pub records: crate::NativeRecords,
    pub store: Arc<dyn ResultStore>,
}
impl crate::snapshot::DeclarationSiblings for SiblingArchives {
    fn siblings(&self, syntax: &ItemSyntax) -> Result<Arc<[Arc<ItemSyntax>]>, QueryError> {
        let refusal = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "front source transport".into(),
                message: "missing or invalid sibling syntax archive".into(),
            })
        };
        let file = self.sources.anchor(syntax.anchor).ok_or_else(refusal)?.0;
        let references = self.files.get(&file).ok_or_else(refusal)?;
        references
            .iter()
            .map(|reference| {
                let bytes = self.store.get_content(*reference).ok_or_else(refusal)?;
                let (mut syntax, occurrence): (ItemSyntax, u32) = self
                    .records
                    .decode(&*self.owners, &bytes)
                    .ok_or_else(refusal)?;
                if self
                    .sources
                    .anchor(syntax.anchor)
                    .is_none_or(|(owner, _)| owner != file)
                    || !syntax.structurally_valid(|span| self.sources.slice(span).is_some())
                {
                    return Err(refusal());
                }
                syntax.occurrence = occurrence;
                Ok(Arc::new(syntax))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Arc::from)
    }
}

/// Distinct immutable source/header backing and front archive/slot backing.
/// The host charges shared source_headers once, alongside the selected S0
/// snapshot; front_inputs is the minimum producer reservation, before outputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrontInputFootprint {
    pub source_headers: u64,
    pub front_inputs: u64,
}
impl FrontInputFootprint {
    pub fn total(self) -> Option<u64> {
        self.source_headers.checked_add(self.front_inputs)
    }
}
impl QueryEngine {
    /// Inspect actual captured input lengths, without body checking or Lower.
    pub fn front_input_footprint(
        &mut self,
        definitions: &[DefPath],
    ) -> Result<FrontInputFootprint, QueryError> {
        let refused = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "front input admission".into(),
                message: "source input size overflow or missing syntax archive".into(),
            })
        };
        for definition in definitions {
            self.materialize(definition)?;
        }
        let store = self
            .result_store
            .clone()
            .filter(|s| s.enabled())
            .ok_or_else(refused)?;
        let records = crate::NativeRecords::new(self.types.clone(), store);
        let owners = crate::NativeRecordContext::capture(self);
        let declarations = self.frozen_declarations();
        let mut front_inputs = FrontBodyQueries::input_bytes(definitions).ok_or_else(refused)?;
        let mut seen = BTreeSet::new();
        let mut files = BTreeSet::new();
        for definition in definitions {
            let syntax = self.snapshot(definition)?;
            let bytes = records
                .encode(&owners, &(&syntax, syntax.occurrence))
                .ok_or_else(refused)?;
            let reference = SOURCE_INPUT.reference(&bytes).ok_or_else(refused)?;
            if seen.insert(reference.output.0) {
                front_inputs = front_inputs
                    .checked_add(bytes.len() as u64)
                    .ok_or_else(refused)?;
            }
            let file = self.sources.anchor(syntax.anchor).ok_or_else(refused)?.0;
            if files.insert(file) {
                for sibling in declarations.siblings(&syntax).iter() {
                    let bytes = records
                        .encode(&owners, &(sibling, sibling.occurrence))
                        .ok_or_else(refused)?;
                    let reference = SOURCE_INPUT.reference(&bytes).ok_or_else(refused)?;
                    if seen.insert(reference.output.0) {
                        front_inputs = front_inputs
                            .checked_add(bytes.len() as u64)
                            .ok_or_else(refused)?;
                    }
                    front_inputs = front_inputs
                        .checked_add(std::mem::size_of::<Reference>() as u64)
                        .ok_or_else(refused)?;
                }
            }
        }
        let source_headers = self
            .sources
            .input_bytes()
            .ok_or_else(refused)?
            .checked_add(self.declarations.observation_bytes().ok_or_else(refused)?)
            .ok_or_else(refused)?
            .checked_add(self.catalog.input_bytes().ok_or_else(refused)?)
            .ok_or_else(refused)?
            .checked_add(owners.input_bytes().ok_or_else(refused)?)
            .ok_or_else(refused)?;
        Ok(FrontInputFootprint {
            source_headers,
            front_inputs,
        })
    }
}
