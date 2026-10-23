//! Ordinary parse heads, separate from semantic products and verifier authority.
//! A hit indexes names and source positions without expression arenas or types.
use super::*;
use crate::snapshot::DeclarationHead;
use bincode::Options;
use serde::{Deserialize, Serialize};

/// Bounds for item-local lexical identities, independent of expression data.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct LexicalShape {
    expressions: u64,
    statements: u64,
    patterns: u64,
    types: u64,
}
impl LexicalShape {
    pub(super) fn capture(syntax: &ItemSyntax) -> Self {
        Self {
            expressions: syntax.tree.expressions.len() as u64,
            statements: syntax.tree.statements.len() as u64,
            patterns: syntax.tree.patterns.len() as u64,
            types: syntax.tree.types.len() as u64,
        }
    }
    pub(super) fn valid_site(self, site: hirsel_types::NameSite) -> bool {
        use hirsel_types::NameSite;
        match site {
            NameSite::Expression(id) => u64::from(id.0) < self.expressions,
            NameSite::Statement(id) => u64::from(id.0) < self.statements,
            NameSite::Pattern(id) => u64::from(id.0) < self.patterns,
            NameSite::Type(id) => u64::from(id.0) < self.types,
            NameSite::Item => true,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Head {
    definition: DefId,
    kind: u8,
    visibility: hirsel_syntax::Visibility,
    base: u32,
    start: u32,
    end: u32,
    header: ContentHash,
    body: ContentHash,
    generator: bool,
    lexical: LexicalShape,
}
#[derive(Serialize, Deserialize)]
struct FileHeads {
    schema: u32,
    context: FileContext,
    source: ContentHash,
    heads: Vec<Head>,
}
#[derive(Clone)]
struct Candidate {
    head: DeclarationHead,
    file: FileId,
    base: u32,
    generator: bool,
    lexical: LexicalShape,
    occurrence: u32,
}
#[derive(Default)]
pub(crate) struct Catalog {
    files: BTreeMap<FileId, FileContext>,
    candidates: BTreeMap<DefPath, Vec<Candidate>>,
}
impl Catalog {
    pub(super) fn lexical(
        &self,
        path: &DefPath,
    ) -> Option<(hirsel_syntax::LexicalOwner, LexicalShape)> {
        let candidate = self.candidates.get(path)?.first()?;
        Some((
            hirsel_syntax::LexicalOwner {
                def: candidate.head.definition,
                occurrence: candidate.occurrence,
            },
            candidate.lexical,
        ))
    }
    pub(crate) fn paths(&self) -> impl Iterator<Item = &DefPath> {
        self.candidates.keys()
    }
    pub(super) fn anchor(&self, path: &DefPath, occurrence: u32) -> Option<hirsel_span::Anchor> {
        Some(
            self.candidates
                .get(path)?
                .iter()
                .find(|c| c.occurrence == occurrence)?
                .head
                .name
                .anchor,
        )
    }
    pub(crate) fn body(&self, path: &DefPath) -> Option<ContentHash> {
        Some(self.candidates.get(path)?.first()?.head.body)
    }
    pub(super) fn storeable(&self, path: &DefPath, kind: QueryKind) -> bool {
        self.candidates
            .get(path)
            .and_then(|c| c.first())
            .is_some_and(|c| {
                if kind == QueryKind::Generate {
                    c.generator
                } else {
                    !c.generator
                }
            })
    }
    pub(crate) fn cited_text(&self, sources: &SourceMap, path: &DefPath) -> Option<ContentHash> {
        let parts: Vec<_> = self
            .candidates
            .get(path)?
            .iter()
            .map(|c| {
                let mut part = sources
                    .file(c.file)
                    .unwrap()
                    .logical_path()
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec();
                part.push(0);
                part.extend(c.occurrence.to_le_bytes());
                part.extend(c.head.name.start.to_le_bytes());
                part.extend(c.head.name.end.to_le_bytes());
                part.extend(c.head.body.0);
                part
            })
            .collect();
        Some(ContentHash::of_parts(
            "hirsel.read.cited",
            parts.iter().map(Vec::as_slice),
        ))
    }
    pub(super) fn file_definitions(&self, file: FileId) -> Option<Vec<DefPath>> {
        self.files.contains_key(&file).then(|| {
            self.candidates
                .iter()
                .filter(|(_, candidates)| candidates.iter().any(|c| c.file == file))
                .map(|(path, _)| path.clone())
                .collect()
        })
    }
}
impl QueryEngine {
    fn catalog_key(&self, file: FileId, context: &FileContext) -> Option<crate::store::EntryKey> {
        self.result_store.as_ref()?;
        let source = self.sources.file(file)?;
        if source.path.to_string_lossy().starts_with('<') {
            return None;
        }
        let scope = bincode::serialize(context).ok()?;
        let e = &self.environment;
        let environment = ContentHash::of_parts(
            "hirsel.parse-heads.environment.v1",
            [
                e.interface_version.to_le_bytes().as_slice(),
                e.compiler_version.as_bytes(),
                e.spec_hash.0.as_slice(),
                e.builtin_signatures_hash.0.as_slice(),
                self.registry.revision().0.as_slice(),
                self.store_identity.0.as_slice(),
                scope.as_slice(),
            ],
        );
        let logical = source.logical_path().as_os_str().as_encoded_bytes();
        let owner = DefPath(ItemPath {
            package: context.package.clone(),
            modules: context.modules.clone(),
            name: format!(
                "$source${}",
                ContentHash::of_bytes(logical)
                    .0
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            ),
        });
        Some(crate::store::EntryKey::new(
            environment,
            &owner,
            ContentHash::of_bytes(source.text().as_bytes()),
            QueryKind::Parse,
            LoweringPolicy::Reachable,
        ))
    }
    /// Index an immutable source's ordinary declaration heads. A hit restores
    /// names and anchors only. Explicit parse/body queries demand syntax later.
    pub fn index_source(&mut self, file: FileId, context: &FileContext) -> Result<(), QueryError> {
        if self.catalog.files.get(&file) == Some(context) {
            return Ok(());
        }
        if let Some((old_context, environment, result)) = self.parsed.get(&file)
            && old_context == context
            && environment == &self.package_environment(&context.package)
        {
            return result.as_ref().map(|_| ()).map_err(Clone::clone);
        }
        if self.catalog.files.contains_key(&file) {
            self.materialize_catalog_file(file)?;
            self.catalog.candidates.retain(|_, candidates| {
                candidates.retain(|candidate| candidate.file != file);
                !candidates.is_empty()
            });
            self.parsed.remove(&file);
        }
        // An edit first makes only its old source and duplicate candidates
        // concrete, preserving the ordinary occurrence/anchor transaction.
        self.prepare_catalog_edit(file)?;
        let key = self.catalog_key(file, context);
        let cached = key.as_ref().and_then(|key| {
            let bytes = self.result_store.as_ref()?.get(key)?;
            let record: FileHeads = bincode::DefaultOptions::new()
                .with_fixint_encoding()
                .with_limit(bytes.len() as u64)
                .reject_trailing_bytes()
                .deserialize(&bytes)
                .ok()?;
            self.valid_heads(file, context, &record).then_some(record)
        });
        if self.verify_store == crate::store::VerifyStore::Off
            && let Some(record) = cached
        {
            self.adopt_heads(file, context, record.heads);
            return Ok(());
        }
        self.parse_file(file, context)?;
        if let Some(key) = key {
            let record = self.capture_heads(file, context);
            let bytes = bincode::serialize(&record).ok();
            if let Some(cached) = cached
                && bincode::serialize(&cached).ok() != bytes
            {
                return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "store verification".into(),
                    message: "parse heads differ from recomputation".into(),
                }));
            }
            if let Some(bytes) = bytes {
                self.result_store.as_ref().unwrap().put(&key, &bytes);
            }
        }
        Ok(())
    }
    fn capture_heads(&self, file: FileId, context: &FileContext) -> FileHeads {
        let mut heads: Vec<_> = self
            .items
            .values()
            .flatten()
            .filter_map(|syntax| {
                let (owner, base) = self.sources.anchor(syntax.anchor)?;
                if owner != file {
                    return None;
                }
                let head = DeclarationHead::capture(&self.sources, syntax);
                Some(Head {
                    definition: head.definition,
                    kind: head.kind,
                    visibility: head.visibility,
                    base,
                    start: head.name.start,
                    end: head.name.end,
                    header: head.header,
                    body: head.body,
                    generator: generation::generator(syntax).is_some(),
                    lexical: LexicalShape::capture(syntax),
                })
            })
            .collect();
        heads.sort_by_key(|h| (h.base, h.definition.path().clone()));
        FileHeads {
            schema: 2,
            context: context.clone(),
            source: ContentHash::of_bytes(self.sources.file(file).unwrap().text().as_bytes()),
            heads,
        }
    }
    fn valid_heads(&self, file: FileId, context: &FileContext, record: &FileHeads) -> bool {
        let Some(source) = self.sources.file(file) else {
            return false;
        };
        record.schema == 2
            && record.context == *context
            && record.source == ContentHash::of_bytes(source.text().as_bytes())
            && record
                .heads
                .windows(2)
                .all(|h| (h[0].base, h[0].definition.path()) < (h[1].base, h[1].definition.path()))
            && record.heads.iter().all(|h| {
                let path = h.definition.path();
                path.0.package == context.package
                    && path.0.modules == context.modules
                    && h.kind <= 3
                    && h.start <= h.end
                    && h.base
                        .checked_add(h.end)
                        .is_some_and(|end| end as usize <= source.text().len())
                    && h.base
                        .checked_add(h.start)
                        .is_some_and(|start| source.text().is_char_boundary(start as usize))
                    && h.base
                        .checked_add(h.end)
                        .is_some_and(|end| source.text().is_char_boundary(end as usize))
            })
    }
    fn adopt_heads(&mut self, file: FileId, context: &FileContext, heads: Vec<Head>) {
        let mut anchors = BTreeMap::new();
        let mut affected = BTreeSet::new();
        for head in heads {
            let anchor = *anchors
                .entry(head.base)
                .or_insert_with(hirsel_span::Anchor::fresh);
            self.sources.move_anchor(anchor, file, head.base);
            let path = head.definition.path().clone();
            affected.insert(path.clone());
            self.catalog
                .candidates
                .entry(path)
                .or_default()
                .push(Candidate {
                    file,
                    base: head.base,
                    generator: head.generator,
                    lexical: head.lexical,
                    occurrence: 0,
                    head: DeclarationHead {
                        definition: head.definition,
                        kind: head.kind,
                        visibility: head.visibility,
                        name: Span {
                            anchor,
                            start: head.start,
                            end: head.end,
                        },
                        header: head.header,
                        body: head.body,
                    },
                });
        }
        for path in affected {
            let candidates = self.catalog.candidates.get_mut(&path).unwrap();
            candidates.sort_by_key(|c| (self.sources.file(c.file).unwrap().path.clone(), c.base));
            let mut heads: Vec<_> = candidates
                .iter()
                .map(|c| {
                    (
                        c.head.clone(),
                        self.sources
                            .file(c.file)
                            .unwrap()
                            .logical_path()
                            .as_os_str()
                            .as_encoded_bytes()
                            .to_vec(),
                    )
                })
                .collect();
            // A source miss may already have installed candidates of this name.
            for syntax in self.items.get(&path).into_iter().flatten() {
                let head = DeclarationHead::capture(&self.sources, syntax);
                if !heads.iter().any(|(h, _)| h.name.anchor == head.name.anchor) {
                    heads.push((head, crate::snapshot::logical(&self.sources, syntax)));
                }
            }
            heads.sort_by_key(|(h, _)| {
                let (file, base) = self.sources.anchor(h.name.anchor).unwrap();
                (self.sources.file(file).unwrap().path.clone(), base)
            });
            for candidate in self.catalog.candidates.get_mut(&path).unwrap() {
                candidate.occurrence = heads
                    .iter()
                    .position(|(h, _)| h.name.anchor == candidate.head.name.anchor)
                    .unwrap() as u32;
            }
            self.declarations.set_heads(&path, &heads);
        }
        self.catalog.files.insert(file, context.clone());
        let revision = self.manifest_revision();
        self.declarations.inputs_mut().manifest_revision = revision;
        self.published_manifest = Some(revision);
        self.retire_snapshot();
    }
    pub(super) fn materialize_catalog(&mut self, path: &DefPath) -> Result<(), QueryError> {
        let files: BTreeSet<_> = self
            .catalog
            .candidates
            .get(path)
            .into_iter()
            .flatten()
            .map(|c| c.file)
            .collect();
        for file in files {
            self.materialize_catalog_file(file)?;
        }
        Ok(())
    }
    pub(super) fn materialize_catalog_file(&mut self, file: FileId) -> Result<(), QueryError> {
        let Some(context) = self.catalog.files.get(&file).cloned() else {
            return Ok(());
        };
        let mut output = self.parse_syntax(file, &context)?;
        if has_errors(&output.diagnostics) {
            return Err(QueryError::Diagnostics(output.diagnostics));
        }
        let mut listed = vec![];
        for syntax in &mut output.value.items {
            let (_, base) = self.sources.anchor(syntax.anchor).unwrap();
            let candidates = self
                .catalog
                .candidates
                .get(&syntax.item.definition)
                .ok_or(QueryError::NotImplemented)?;
            let candidate = candidates
                .iter()
                .find(|c| c.file == file && c.base == base)
                .ok_or(QueryError::NotImplemented)?;
            let occurrence = candidate.occurrence;
            if syntax.text_hash != candidate.head.body
                || self.header_hash(syntax) != candidate.head.header
                || LexicalShape::capture(syntax) != candidate.lexical
            {
                return Err(QueryError::NotImplemented);
            }
            let anchor = candidate.head.name.anchor;
            let syntax = Arc::make_mut(syntax);
            syntax.occurrence = occurrence;
            syntax.reanchor(anchor);
            self.sources.move_anchor(anchor, file, base);
        }
        for syntax in &output.value.items {
            let mut add = |item: Arc<ItemSyntax>| {
                let candidates = self.items.entry(item.item.definition.clone()).or_default();
                candidates.push(item.clone());
                candidates.sort_by_key(|c| c.occurrence);
                listed.push(item);
            };
            if let Some(helper) = crate::facts::snapshot(syntax) {
                add(helper);
            }
            for helper in crate::dictionaries::literal_snapshots(syntax) {
                add(helper);
            }
            add(syntax.clone());
        }
        self.declarations.place_file(file, listed, &self.sources);
        self.catalog.files.remove(&file);
        self.catalog.candidates.retain(|_, candidates| {
            candidates.retain(|candidate| candidate.file != file);
            !candidates.is_empty()
        });
        self.parsed.insert(
            file,
            (
                context.clone(),
                self.package_environment(&context.package),
                Ok(output),
            ),
        );
        Ok(())
    }
    pub(super) fn prepare_catalog_edit(&mut self, file: FileId) -> Result<(), QueryError> {
        let Some(source) = self.sources.file(file) else {
            return Ok(());
        };
        let old: BTreeSet<_> = self
            .catalog
            .candidates
            .values()
            .flatten()
            .filter(|c| {
                c.file != file
                    && self
                        .sources
                        .file(c.file)
                        .is_some_and(|s| s.path == source.path)
            })
            .map(|c| c.file)
            .collect();
        if old.is_empty() {
            return Ok(());
        }
        let affected: Vec<_> = self
            .catalog
            .candidates
            .iter()
            .filter(|(_, c)| c.iter().any(|c| old.contains(&c.file)))
            .map(|(p, _)| p.clone())
            .collect();
        for path in &affected {
            self.materialize_catalog(path)?;
        }
        self.catalog.candidates.retain(|_, candidates| {
            candidates.retain(|c| !old.contains(&c.file));
            !candidates.is_empty()
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ResultStore;

    #[test]
    fn malformed_parse_heads_refuse_before_namespace_or_syntax_adoption() {
        crate::with_compiler_stack(|| {
            let context = default_context();
            let path = DefPath(ItemPath {
                package: context.package.clone(),
                modules: vec![],
                name: "value".into(),
            });
            let source = "fn value() {}\n";
            for mutation in 0..9 {
                let store = Arc::new(crate::store::MemStore::default());
                let mut engine = QueryEngine::new(default_environment(), PhaseAdapters::default())
                    .with_result_store(store.clone());
                let file = engine.sources.add("heads.hir", source.into()).unwrap();
                let key = engine.catalog_key(file, &context).unwrap();
                let mut record = FileHeads {
                    schema: 2,
                    context: context.clone(),
                    source: ContentHash::of_bytes(source.as_bytes()),
                    heads: vec![Head {
                        definition: DefId::intern(&path),
                        kind: 0,
                        visibility: hirsel_syntax::Visibility::Private,
                        base: 0,
                        start: 3,
                        end: 8,
                        header: ContentHash::default(),
                        body: ContentHash::default(),
                        generator: false,
                        lexical: LexicalShape {
                            expressions: 0,
                            statements: 0,
                            patterns: 0,
                            types: 0,
                        },
                    }],
                };
                assert!(engine.valid_heads(file, &context, &record));
                match mutation {
                    0 => record.schema += 1,
                    1 => record.source = ContentHash::default(),
                    2 => record.context.modules.push("foreign".into()),
                    3 => record.heads[0].base = u32::MAX,
                    4 => record.heads[0].end = u32::MAX,
                    5 => record.heads[0].start = record.heads[0].end + 1,
                    6 => record.heads[0].kind = u8::MAX,
                    7 => record.heads.push(record.heads[0].clone()),
                    8 => record.heads[0].definition = DefId::intern(&DefPath::core("foreign")),
                    _ => unreachable!(),
                }
                store.put(&key, &bincode::serialize(&record).unwrap());
                assert!(
                    engine.index_source(file, &context).is_err(),
                    "mutation {mutation}"
                );
                assert!(engine.catalog.candidates.is_empty() && engine.catalog.files.is_empty());
                assert!(engine.declarations.paths().next().is_none());
                assert_eq!(engine.work.parsed_definitions, 0);
            }
        });
    }
}
