use super::*;
use crate::store::{Domain, Reference};
const SOURCE_INPUT: Domain = Domain {
    kind: 0x4600_0013,
    schema: 1,
};
enum Transport {
    Stored {
        files: BTreeMap<FileId, Vec<Reference>>,
        records: crate::NativeRecords,
        owners: Arc<crate::NativeRecordContext>,
        store: Arc<dyn crate::store::ResultStore>,
    },
    Written(BTreeMap<FileId, Vec<Arc<ItemSyntax>>>),
}
pub(super) struct HeaderSources {
    by_definition: hirsel_syntax::DefMap<Vec<FileId>>,
    facts: hirsel_syntax::DefMap<Arc<catalog::SourceFacts>>,
    transport: std::sync::RwLock<Transport>,
}
impl HeaderSources {
    pub(super) fn capture(engine: &QueryEngine) -> Self {
        let mut files = BTreeMap::<_, Vec<_>>::new();
        let mut by_definition = hirsel_syntax::DefMap::<Vec<FileId>>::default();
        for (path, candidates) in &engine.items {
            let mut selected = BTreeSet::new();
            for syntax in candidates {
                let file = engine.item_file(syntax);
                selected.insert(file);
                files.entry(file).or_default().push(syntax.clone());
            }
            by_definition.insert(DefId::intern(path), selected.into_iter().collect());
        }
        let facts = engine
            .items
            .iter()
            .filter_map(|(path, candidates)| {
                Some((
                    DefId::intern(path),
                    engine.catalog.shared_syntax_facts(candidates.first()?),
                ))
            })
            .collect();
        Self {
            by_definition,
            facts,
            transport: std::sync::RwLock::new(Transport::Written(files)),
        }
    }
    pub(super) fn facts(&self, id: DefId) -> Option<catalog::SourceFacts> {
        self.facts.get(&id).map(|facts| (**facts).clone())
    }
    pub(super) fn facts_index_bytes(definitions: usize) -> u64 {
        let mut weight = crate::NativeMetadataWeight::default();
        weight.tree::<DefId, Arc<catalog::SourceFacts>>(definitions);
        weight.bytes()
    }
    pub(super) fn archive(
        &self,
        types: Arc<hirsel_types::TypeInterner>,
        owners: Arc<crate::NativeRecordContext>,
        store: Arc<dyn crate::store::ResultStore>,
    ) -> Result<(), QueryError> {
        let mut transport = self.transport.write().unwrap_or_else(|e| e.into_inner());
        let Transport::Written(written) = &*transport else {
            return Ok(());
        };
        let records = crate::NativeRecords::new(types, store.clone());
        let refused = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "header source transport".into(),
                message: "indexed source exceeds admitted input transport".into(),
            })
        };
        let files = written
            .iter()
            .map(|(file, syntaxes)| {
                let references = syntaxes
                    .iter()
                    .map(|syntax| {
                        records
                            .encode(&*owners, &(syntax, syntax.occurrence))
                            .and_then(|bytes| store.put_content(SOURCE_INPUT, &bytes))
                            .ok_or_else(refused)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((*file, references))
            })
            .collect::<Result<BTreeMap<_, _>, QueryError>>()?;
        *transport = Transport::Stored {
            files,
            records,
            owners,
            store,
        };
        Ok(())
    }
    pub(super) fn materialize(
        &self,
        engine: &mut QueryEngine,
        path: &DefPath,
    ) -> Result<bool, QueryError> {
        if engine.items.contains_key(path) {
            return Ok(true);
        }
        let Some(files) = self.by_definition.get(&DefId::intern(path)) else {
            return Ok(false);
        };
        let refused = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "header source transport".into(),
                message: format!("cannot decode indexed source for {path}"),
            })
        };
        for file in files {
            let transport = self.transport.read().unwrap_or_else(|e| e.into_inner());
            let syntaxes = match &*transport {
                Transport::Written(files) => files.get(file).ok_or_else(refused)?.clone(),
                Transport::Stored {
                    files,
                    records,
                    owners,
                    store,
                } => files
                    .get(file)
                    .ok_or_else(refused)?
                    .iter()
                    .map(|reference| {
                        let bytes = store.get_content(*reference).ok_or_else(refused)?;
                        let (mut syntax, occurrence): (ItemSyntax, u32) =
                            records.decode(&**owners, &bytes).ok_or_else(refused)?;
                        if !syntax.structurally_valid(|span| engine.sources.slice(span).is_some())
                            || engine
                                .sources
                                .anchor(syntax.anchor)
                                .is_none_or(|(owner, _)| owner != *file)
                        {
                            return Err(refused());
                        }
                        syntax.occurrence = occurrence;
                        Ok(Arc::new(syntax))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            };
            for syntax in &syntaxes {
                let candidates = engine
                    .items
                    .entry(syntax.item.definition.clone())
                    .or_default();
                if !candidates
                    .iter()
                    .any(|old| old.anchor == syntax.anchor && old.occurrence == syntax.occurrence)
                {
                    candidates.push(syntax.clone());
                    candidates.sort_by_key(|s| s.occurrence);
                }
            }
            engine
                .declarations
                .place_file(*file, syntaxes, &engine.sources);
        }
        Ok(true)
    }
}
