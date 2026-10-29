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
        let refusal = |detail: &str| {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "front source transport".into(),
                message: format!("{}: {detail}", syntax.item.definition),
            })
        };
        let file = self
            .sources
            .anchor(syntax.anchor)
            .ok_or_else(|| refusal("missing source anchor"))?
            .0;
        let references = self
            .files
            .get(&file)
            .ok_or_else(|| refusal("missing sibling file inventory"))?;
        references
            .iter()
            .map(|reference| {
                let bytes = self
                    .store
                    .get_content(*reference)
                    .ok_or_else(|| refusal("missing sibling content"))?;
                let (mut syntax, occurrence): (ItemSyntax, u32) = self
                    .records
                    .decode(&*self.owners, &bytes)
                    .ok_or_else(|| refusal("invalid sibling codec"))?;
                let (owner, base) = self
                    .sources
                    .anchor(syntax.anchor)
                    .ok_or_else(|| refusal("missing sibling anchor"))?;
                let text = self
                    .sources
                    .file(owner)
                    .ok_or_else(|| refusal("missing sibling source"))?
                    .text();
                // Structural validation also checks that every span uses this
                // item's anchor. Resolve its file/base once for the whole tree.
                if !syntax.structurally_valid(|span| {
                    text.get(
                        base.saturating_add(span.start) as usize
                            ..base.saturating_add(span.end) as usize,
                    )
                    .is_some()
                }) {
                    return Err(refusal("invalid sibling spans"));
                }
                syntax.occurrence = occurrence;
                Ok(Arc::new(syntax))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Arc::from)
    }
}

/// Distinct immutable source/header backing and front reference/slot backing.
/// The host charges shared source_headers once, alongside the selected S0
/// snapshot; front_inputs is the minimum producer reservation, before output metadata.
/// Archive byte ownership remains with the selected provider.
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
    pub(super) fn front_sibling_inputs(
        &self,
        definitions: &[DefPath],
    ) -> Result<BTreeMap<FileId, Arc<ItemSyntax>>, QueryError> {
        let mut files = BTreeMap::new();
        for definition in definitions {
            let syntax = self.snapshot(definition)?;
            let file = self
                .sources
                .anchor(syntax.anchor)
                .ok_or(QueryError::NotImplemented)?
                .0;
            files.entry(file).or_insert(syntax);
        }
        // Private generator constants share their declaration's written file.
        // That file can be absent from the ordinary P inputs during bootstrap.
        for syntax in self.items.values().flatten() {
            if generation::generator(syntax).is_some() {
                let file = self
                    .sources
                    .anchor(syntax.anchor)
                    .ok_or(QueryError::NotImplemented)?
                    .0;
                files.entry(file).or_insert_with(|| syntax.clone());
            }
        }
        Ok(files)
    }
    /// Bound front backing from indexed metadata. Planning must not demand
    /// syntax or freeze declarations. Source owners use their existing
    /// immutable indexed product, including its path allocations.
    pub fn front_input_footprint(
        &self,
        definitions: &[DefPath],
    ) -> Result<FrontInputFootprint, QueryError> {
        let refused = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "front input admission".into(),
                message: "source input size overflow or missing syntax archive".into(),
            })
        };
        if self
            .result_store
            .as_ref()
            .is_none_or(|store| !store.enabled())
        {
            return Err(refused());
        }
        let (files, siblings) = self.declarations.sibling_inventory();
        // Lazy files already own declaration heads and source anchors. Their
        // eventual syntax capture cannot require more rows than that index.
        let (lazy_files, lazy_siblings) = self.catalog.sibling_inventory().ok_or_else(refused)?;
        let files = files.checked_add(lazy_files).ok_or_else(refused)?;
        let siblings = siblings.checked_add(lazy_siblings).ok_or_else(refused)?;
        let mut index = crate::NativeMetadataWeight::default();
        index.tree::<FileId, Vec<Reference>>(files);
        let front_inputs = FrontBodyQueries::input_bytes(definitions)
            .and_then(|bytes| bytes.checked_add(index.bytes()))
            .and_then(|bytes| {
                bytes.checked_add(
                    (siblings as u64).checked_mul(std::mem::size_of::<Reference>() as u64)?,
                )
            })
            .ok_or_else(refused)?;
        let owner_bytes = crate::NativeRecordContext::capture(self)
            .input_bytes()
            .ok_or_else(refused)?;
        let source_headers = self
            .sources
            .input_bytes()
            .ok_or_else(refused)?
            .checked_add(self.declarations.observation_bytes().ok_or_else(refused)?)
            .ok_or_else(refused)?
            .checked_add(self.catalog.input_bytes().ok_or_else(refused)?)
            .ok_or_else(refused)?
            .checked_add(owner_bytes)
            .ok_or_else(refused)?
            .checked_add(header_products::HeaderProducts::source_fact_index_bytes(
                self.source_paths().len(),
                self.adapters
                    .modes
                    .as_ref()
                    .is_some_and(|modes| modes.infers_private_unique()),
            ))
            .ok_or_else(refused)?;
        Ok(FrontInputFootprint {
            source_headers,
            front_inputs,
        })
    }
}
