use super::*;
use crate::store::{Domain, Reference};
const SOURCE_INPUT: Domain = Domain {
    kind: 0x4600_0013,
    schema: 1,
};
enum Transport {
    Stored {
        files: BTreeMap<FileId, Vec<Option<Reference>>>,
        records: crate::NativeRecords,
        owners: Arc<crate::NativeRecordContext>,
        store: Arc<dyn crate::store::ResultStore>,
    },
    Written(BTreeMap<FileId, Vec<Arc<ItemSyntax>>>),
}
pub(super) struct HeaderSources {
    by_definition: hirsel_syntax::DefMap<Vec<FileId>>,
    transport: Transport,
}
impl HeaderSources {
    pub(super) fn capture(engine: &QueryEngine, owners: Arc<crate::NativeRecordContext>) -> Self {
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
        let transport = match engine.result_store.clone().filter(|store| store.enabled()) {
            Some(store) => {
                let records = crate::NativeRecords::new(engine.types.clone(), store.clone());
                let files = files
                    .into_iter()
                    .map(|(file, syntaxes)| {
                        (
                            file,
                            syntaxes
                                .into_iter()
                                .map(|syntax| {
                                    records
                                        .encode(&*owners, &(&syntax, syntax.occurrence))
                                        .and_then(|bytes| store.put_content(SOURCE_INPUT, &bytes))
                                })
                                .collect(),
                        )
                    })
                    .collect();
                Transport::Stored {
                    files,
                    records,
                    owners,
                    store,
                }
            }
            None => Transport::Written(files),
        };
        Self {
            by_definition,
            transport,
        }
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
            let syntaxes = match &self.transport {
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
                        let bytes = store
                            .get_content(reference.ok_or_else(refused)?)
                            .ok_or_else(refused)?;
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
