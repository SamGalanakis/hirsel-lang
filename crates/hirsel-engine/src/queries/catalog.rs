//! Ordinary parse heads, separate from semantic products and verifier authority.
//! A hit indexes names and source positions without expression arenas or types.
use super::*;
mod syntax;
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
/// Declaration observations needed by checking without retaining syntax.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct SourceFacts {
    pub(crate) generator: bool,
    pub(crate) instance_bodied: bool,
    pub(crate) executable: bool,
    pub(crate) body: bool,
    pub(crate) constant: bool,
    pub(crate) execution: bool,
    pub(crate) comptime: Vec<(u32, u32, u32)>,
    pub(crate) fact: bool,
    pub(crate) associations: Vec<(u8, u32, u32)>,
    pub(crate) arms: Vec<(String, u32, u32)>,
}
impl SourceFacts {
    pub(crate) fn capture(syntax: &ItemSyntax) -> Self {
        use hirsel_syntax::{AttributeArgument, TypeBody, Visibility};
        let item = &syntax.item;
        let constant = matches!(item.kind, ItemKind::Const { .. });
        let arms = match &item.kind {
            ItemKind::Type {
                body: TypeBody::Variants(arms),
                ..
            } if !arms.is_empty() && item.visibility == Visibility::Private => {
                let names = super::dead_arms::arm_names(syntax);
                if names.len() == arms.len() {
                    names
                        .into_iter()
                        .map(|(name, span)| (name, span.start, span.end))
                        .collect()
                } else {
                    vec![]
                }
            }
            _ => vec![],
        };
        let comptime: Vec<_> = syntax
            .tree
            .expressions
            .iter()
            .enumerate()
            .filter_map(|(index, e)| {
                matches!(e.kind, hirsel_syntax::ExprKind::Comptime(_)).then_some((
                    index as u32,
                    e.origin.span.start,
                    e.origin.span.end,
                ))
            })
            .collect();
        Self {
            generator: generation::generator(syntax).is_some(),
            instance_bodied: generation::instance_bodied(syntax),
            executable: generation::executable(syntax),
            body: constant || matches!(item.kind, ItemKind::Function { body: Some(_), .. }),
            constant,
            execution: constant || !comptime.is_empty(),
            comptime,
            fact: matches!(
                item.kind,
                ItemKind::Type {
                    body: TypeBody::Distinct { fact: Some(_), .. },
                    ..
                }
            ),
            associations: item
                .attributes
                .iter()
                .filter_map(|attribute| {
                    let name = attribute.name.as_str();
                    if !matches!(name, "loop_source" | "format")
                        && !matches!(
                            attribute.arguments.as_slice(),
                            [AttributeArgument::SerializableCheck(_)]
                        )
                    {
                        return None;
                    }
                    let kind = if name == "format" {
                        0
                    } else if name == "serializable" {
                        1
                    } else {
                        2
                    };
                    Some((kind, attribute.origin.span.start, attribute.origin.span.end))
                })
                .collect(),
            arms,
        }
    }
}
#[derive(Clone)]
pub(crate) struct SourceItem {
    pub(crate) definition: DefPath,
    pub(crate) anchor: hirsel_span::Anchor,
    pub(crate) facts: SourceFacts,
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
    facts: SourceFacts,
    lexical: LexicalShape,
}
#[derive(Serialize, Deserialize)]
struct FileHeads {
    schema: u32,
    context: FileContext,
    source: ContentHash,
    heads: Vec<Head>,
    integrity: ContentHash,
}
fn heads_integrity(heads: &[Head]) -> ContentHash {
    ContentHash::of_parts(
        "hirsel.parse-heads.payload.v5",
        [&bincode::serialize(heads).expect("source heads encode")[..]],
    )
}
#[derive(Clone)]
struct Candidate {
    head: DeclarationHead,
    file: FileId,
    base: u32,
    facts: SourceFacts,
    lexical: LexicalShape,
    occurrence: u32,
}
#[derive(Default)]
pub(crate) struct Catalog {
    files: BTreeMap<FileId, FileContext>,
    candidates: BTreeMap<DefPath, Vec<Candidate>>,
    facts_cache:
        std::sync::RwLock<BTreeMap<hirsel_syntax::LexicalOwner, (ContentHash, SourceFacts)>>,
}
impl Clone for Catalog {
    fn clone(&self) -> Self {
        Self {
            files: self.files.clone(),
            candidates: self.candidates.clone(),
            // Facts are recomputable memoization, not source observations.
            facts_cache: Default::default(),
        }
    }
}
impl Catalog {
    pub(super) fn retained_payload_bytes(&self) -> u64 {
        let mut weight = crate::NativeMetadataWeight::default();
        let facts = |weight: &mut crate::NativeMetadataWeight, facts: &SourceFacts| {
            weight.flat_vec(&facts.comptime);
            weight.flat_vec(&facts.associations);
            weight.vec(&facts.arms, |w, (name, _, _)| w.add(name.capacity()));
        };
        weight.map(&self.files, |w, _, context| {
            w.add(context.package.name.capacity());
            w.add(context.package.version.capacity());
            w.vec(&context.modules, |w, module| w.add(module.capacity()));
        });
        weight.map(&self.candidates, |w, path, candidates| {
            w.path(path);
            w.vec(candidates, |w, candidate| facts(w, &candidate.facts));
        });
        weight.map(
            &self.facts_cache.read().unwrap_or_else(|e| e.into_inner()),
            |w, _, (_, value)| facts(w, value),
        );
        weight.bytes()
    }
    pub(super) fn input_bytes(&self) -> Option<u64> {
        let mut n = std::mem::size_of::<Self>() as u64;
        for candidates in self.candidates.values() {
            n = n.checked_add(
                (candidates.capacity() * std::mem::size_of::<Candidate>()
                    + 8 * std::mem::size_of::<usize>()) as u64,
            )?;
            for candidate in candidates {
                let facts = &candidate.facts;
                n = n.checked_add(
                    (facts.comptime.capacity() * std::mem::size_of::<(u32, u32, u32)>()
                        + facts.associations.capacity() * std::mem::size_of::<(u8, u32, u32)>()
                        + facts.arms.capacity() * std::mem::size_of::<(String, u32, u32)>())
                        as u64,
                )?;
                for (arm, _, _) in &facts.arms {
                    n = n.checked_add(arm.capacity() as u64)?;
                }
            }
        }
        n.checked_add(
            (self.files.len()
                * (std::mem::size_of::<(FileId, FileContext)>() + 6 * std::mem::size_of::<usize>()))
                as u64,
        )
    }

    pub(super) fn clear_facts(&self) {
        self.facts_cache.write().unwrap().clear();
    }
    fn syntax_facts(&self, syntax: &ItemSyntax) -> SourceFacts {
        if let Some((text, facts)) = self.facts_cache.read().unwrap().get(&syntax.owner())
            && *text == syntax.text_hash
        {
            return facts.clone();
        }
        let facts = SourceFacts::capture(syntax);
        self.facts_cache
            .write()
            .unwrap()
            .insert(syntax.owner(), (syntax.text_hash, facts.clone()));
        facts
    }
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
    pub(super) fn facts(&self, path: &DefPath) -> Option<&SourceFacts> {
        Some(&self.candidates.get(path)?.first()?.facts)
    }
    /// Ordinary heads cannot become generic generators through syntax
    /// shadowing. Consult the live syntax map only for positive candidates.
    pub(super) fn has_instance_bodied(&self, mut shadowed: impl FnMut(&DefPath) -> bool) -> bool {
        self.candidates.iter().any(|(path, candidates)| {
            candidates
                .first()
                .is_some_and(|candidate| candidate.facts.instance_bodied)
                && !shadowed(path)
        })
    }

    pub(super) fn source_items(&self, file: FileId) -> Option<Vec<SourceItem>> {
        self.files.contains_key(&file).then(|| {
            let mut items: Vec<_> = self
                .candidates
                .iter()
                .flat_map(|(path, candidates)| {
                    candidates
                        .iter()
                        .filter(move |c| c.file == file)
                        .map(move |c| {
                            (
                                c.base,
                                SourceItem {
                                    definition: path.clone(),
                                    anchor: c.head.name.anchor,
                                    facts: c.facts.clone(),
                                },
                            )
                        })
                })
                .collect();
            items.sort_by_key(|(base, item)| (*base, item.definition.clone()));
            items
                .into_iter()
                .map(|(_, item)| item)
                .filter(|item| {
                    hirsel_check_protocol::fact::fact_owner(&item.definition).is_none()
                        && !item.definition.0.name.starts_with("$dictionary$")
                        && !crate::families::derived(&item.definition)
                })
                .collect()
        })
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
    pub(super) fn owner(&self, anchor: hirsel_span::Anchor) -> Option<(DefPath, u32)> {
        self.candidates.iter().find_map(|(path, candidates)| {
            candidates
                .iter()
                .find(|c| c.head.name.anchor == anchor)
                .map(|c| (path.clone(), c.occurrence))
        })
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
                    c.facts.generator
                } else {
                    !c.facts.generator
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
            let mut definitions: Vec<_> = self
                .candidates
                .iter()
                .flat_map(|(path, candidates)| {
                    candidates
                        .iter()
                        .filter(move |c| c.file == file)
                        .map(move |c| (c.base, path.clone()))
                })
                .collect();
            definitions.sort();
            definitions.into_iter().map(|(_, path)| path).collect()
        })
    }
}
impl QueryEngine {
    pub(crate) fn comptime_site(
        &mut self,
        path: &DefPath,
        expression: hirsel_syntax::ExprId,
    ) -> Result<Span, QueryError> {
        if let Some(facts) = self.source_facts(path)
            && let Ok(index) = facts
                .comptime
                .binary_search_by_key(&expression.0, |site| site.0)
        {
            let (_, start, end) = facts.comptime[index];
            let (owner, _) = self.lexical_shape(path).ok_or(QueryError::NotImplemented)?;
            let anchor = self
                .snapshot(path)
                .ok()
                .map(|syntax| syntax.anchor)
                .or_else(|| self.catalog.anchor(path, owner.occurrence))
                .ok_or(QueryError::NotImplemented)?;
            return Ok(Span { anchor, start, end });
        }
        let syntax = self.parse_definition(path)?.value;
        syntax
            .tree
            .expressions
            .get(expression.0 as usize)
            .map(|e| e.origin.span)
            .ok_or(QueryError::NotImplemented)
    }
    pub(super) fn lexical_shape(
        &self,
        path: &DefPath,
    ) -> Option<(hirsel_syntax::LexicalOwner, LexicalShape)> {
        self.snapshot(path)
            .ok()
            .map(|syntax| (syntax.owner(), LexicalShape::capture(&syntax)))
            .or_else(|| self.catalog.lexical(path))
    }
    pub(crate) fn retained_source_syntax(&self, file: FileId) -> Vec<&Arc<ItemSyntax>> {
        if !self.indexed_sources.contains_key(&file) {
            return vec![];
        }
        let mut items: Vec<_> = self
            .items
            .values()
            .flatten()
            .filter(|syntax| {
                let path = &syntax.item.definition;
                self.sources
                    .anchor(syntax.anchor)
                    .is_some_and(|(owner, _)| owner == file)
                    && hirsel_check_protocol::fact::fact_owner(path).is_none()
                    && !path.0.name.starts_with("$dictionary$")
                    && !crate::families::derived(path)
            })
            .collect();
        items.sort_by_key(|syntax| self.sources.anchor(syntax.anchor).unwrap().1);
        items
    }
    pub(crate) fn source_items(&self, file: FileId) -> Vec<SourceItem> {
        if let Some(items) = self.catalog.source_items(file) {
            return items;
        }
        if let Some((_, _, Ok(parsed))) = self.parsed.get(&file) {
            return parsed
                .value
                .items
                .iter()
                .map(|syntax| SourceItem {
                    definition: syntax.item.definition.clone(),
                    anchor: syntax.anchor,
                    facts: self.catalog.syntax_facts(syntax),
                })
                .collect();
        }
        self.retained_source_syntax(file)
            .into_iter()
            .map(|syntax| SourceItem {
                definition: syntax.item.definition.clone(),
                anchor: syntax.anchor,
                facts: self.catalog.syntax_facts(syntax),
            })
            .collect()
    }
    pub(crate) fn source_facts(&self, path: &DefPath) -> Option<SourceFacts> {
        self.items
            .get(path)
            .and_then(|items| items.first())
            .map(|syntax| self.catalog.syntax_facts(syntax))
            .or_else(|| self.catalog.facts(path).cloned())
    }
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
        let environment = self.package_environment(&context.package);
        if let Some((previous, inputs)) = self.indexed_sources.get_mut(&file)
            && previous == context
            && inputs.interface_version == environment.interface_version
            && inputs.compiler_version == environment.compiler_version
            && inputs.spec_hash == environment.spec_hash
            && inputs.builtin_signatures_hash == environment.builtin_signatures_hash
        {
            // The immutable source and its parse context did not change.
            // Manifest-only edits change semantic keys, not declaration heads;
            // adopting those same heads again would duplicate retained items.
            inputs.package_manifest_hash = environment.package_manifest_hash;
            return Ok(());
        }
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
            self.materialize_source(file)?;
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
            self.indexed_sources
                .insert(file, (context.clone(), environment));
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
                    facts: self.catalog.syntax_facts(syntax),
                    lexical: LexicalShape::capture(syntax),
                })
            })
            .collect();
        heads.sort_by_key(|h| (h.base, h.definition.path().clone()));
        FileHeads {
            schema: 5,
            integrity: heads_integrity(&heads),
            context: context.clone(),
            source: ContentHash::of_bytes(self.sources.file(file).unwrap().text().as_bytes()),
            heads,
        }
    }
    fn valid_heads(&self, file: FileId, context: &FileContext, record: &FileHeads) -> bool {
        let Some(source) = self.sources.file(file) else {
            return false;
        };
        record.schema == 5
            && record.integrity == heads_integrity(&record.heads)
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
                    && h.kind <= 5
                    && h.start <= h.end
                    && h.facts.comptime.windows(2).all(|w| w[0].0 < w[1].0)
                    && h.facts.comptime.iter().all(|(id, start, end)| {
                        start <= end
                            && h.base
                                .checked_add(*end)
                                .is_some_and(|end| end as usize <= source.text().len())
                            && h.lexical.valid_site(hirsel_types::NameSite::Expression(
                                hirsel_syntax::ExprId(*id),
                            ))
                    })
                    && h.facts.associations.iter().all(|(kind, start, end)| {
                        *kind <= 2
                            && start <= end
                            && h.base
                                .checked_add(*end)
                                .is_some_and(|end| end as usize <= source.text().len())
                    })
                    && h.facts.arms.iter().all(|(_, start, end)| {
                        start <= end
                            && h.base
                                .checked_add(*end)
                                .is_some_and(|end| end as usize <= source.text().len())
                    })
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
                    facts: head.facts,
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
            self.source_growth.changed(DefId::intern(&path), true);
            self.front_changes.changed(DefId::intern(&path), true);
        }
        self.catalog.files.insert(file, context.clone());
        let revision = self.manifest_revision();
        self.declarations.inputs_mut().manifest_revision = revision;
        self.published_manifest = Some(revision);
        self.retire_snapshot();
    }
    pub(crate) fn materialize(&mut self, path: &DefPath) -> Result<(), QueryError> {
        if !self.items.contains_key(path)
            && let Some((base, bindings)) = hirsel_types::specialization_parts(path)
        {
            return self.materialize_specialization(path, &base, &bindings);
        }
        let files: BTreeSet<_> = self
            .catalog
            .candidates
            .get(path)
            .into_iter()
            .flatten()
            .map(|c| c.file)
            .collect();
        for file in files {
            self.materialize_source(file)?;
        }
        Ok(())
    }
    /// Install the specialization item a closed `with` clause names (GEN-6).
    fn materialize_specialization(
        &mut self,
        path: &DefPath,
        base: &DefPath,
        bindings: &[(String, DefPath)],
    ) -> Result<(), QueryError> {
        self.materialize(base)?;
        let mut implementations = vec![];
        for (binder, implementation) in bindings {
            self.materialize(implementation)?;
            let syntax = self
                .items
                .get(implementation)
                .and_then(|i| i.first())
                .cloned();
            implementations.push((binder.clone(), syntax));
        }
        let unavailable = |message: &str| {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "specialization".into(),
                message: format!("{path}: {message}"),
            })
        };
        let Some(base_syntax) = self.items.get(base).and_then(|i| i.first()).cloned() else {
            return Err(unavailable("its declaration is not indexed"));
        };
        let Some(syntax) = crate::families::specialization(path, &base_syntax, &implementations)
        else {
            return Err(unavailable(
                "its bindings do not name the declaration's binders",
            ));
        };
        self.items.insert(path.clone(), vec![syntax]);
        // A new definition, as a CT-5 instance: the signature snapshot now
        // includes it.
        self.note_inputs([path.clone()]);
        self.retire_snapshot();
        Ok(())
    }
    pub(crate) fn materialize_source(&mut self, file: FileId) -> Result<(), QueryError> {
        let Some(context) = self.catalog.files.get(&file).cloned() else {
            return Ok(());
        };
        let cached = self.load_parse_syntax(file, &context);
        let restored = cached.is_some() && self.verify_store == crate::store::VerifyStore::Off;
        let mut output = if restored {
            cached.unwrap()
        } else {
            self.parse_syntax(file, &context)?
        };
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
                || SourceFacts::capture(syntax) != candidate.facts
            {
                return Err(QueryError::NotImplemented);
            }
            let anchor = candidate.head.name.anchor;
            let syntax = Arc::make_mut(syntax);
            syntax.occurrence = occurrence;
            syntax.reanchor(anchor);
            self.sources.move_anchor(anchor, file, base);
        }
        if !restored {
            self.store_parse_syntax(file, &context, &output)?;
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
            for helper in crate::families::snapshots(syntax) {
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
        let environment = self.package_environment(&context.package);
        self.indexed_sources
            .insert(file, (context.clone(), environment.clone()));
        self.parsed
            .insert(file, (context.clone(), environment, Ok(output)));
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
            self.materialize(path)?;
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
    fn parse_heads_reuse_signature_and_implementation_declarations_and_reject_unknown_kinds() {
        crate::with_compiler_stack(|| {
            let store = Arc::new(crate::store::MemStore::default());
            let create = || {
                QueryEngine::new(default_environment(), crate::phases::engine_only_adapters())
                    .with_result_store(store.clone())
            };
            let context = default_context();
            let text = "signature Value { fn get()->u8 uses {} }\nfn get()->u8{1u8}\nimplementation ops: Value { get = get }\n";
            let mut cold = create();
            let file = cold.sources.add("families.hir", text.into()).unwrap();
            cold.index_source(file, &context).unwrap();
            let paths = cold.indexed_definitions();
            let mut record = cold.capture_heads(file, &context);
            assert!(record.heads.iter().any(|h| h.kind == 4));
            assert!(record.heads.iter().any(|h| h.kind == 5));
            assert!(cold.valid_heads(file, &context, &record));
            record.heads[0].kind = 6;
            record.integrity = heads_integrity(&record.heads);
            assert!(!cold.valid_heads(file, &context, &record));
            let mut warm = create();
            let file = warm.sources.add("families.hir", text.into()).unwrap();
            warm.index_source(file, &context).unwrap();
            assert!(warm.parsed.is_empty());
            assert!(warm.items.is_empty());
            assert_eq!(warm.indexed_definitions(), paths);
        });
    }

    #[test]
    fn manifest_only_edits_do_not_adopt_duplicate_heads_over_retained_syntax() {
        crate::with_compiler_stack(|| {
            let mut engine =
                QueryEngine::new(default_environment(), crate::phases::engine_only_adapters())
                    .with_result_store(Arc::new(crate::store::MemStore::default()));
            let context = default_context();
            let file = engine
                .sources
                .add("manifest.hir", "fn value()->u8{1u8}\n".into())
                .unwrap();
            engine.index_source(file, &context).unwrap();
            let path = engine.indexed_definitions().pop().unwrap();
            let anchor = engine.declarations.sites(&path)[0].name_span.anchor;
            for revision in ["first", "second"] {
                engine.environment.package_manifest_hash =
                    ContentHash::of_bytes(revision.as_bytes());
                engine.index_source(file, &context).unwrap();
                assert_eq!(engine.declarations.sites(&path).len(), 1);
                assert_eq!(engine.declarations.sites(&path)[0].name_span.anchor, anchor);
                assert_eq!(engine.items[&path].len(), 1);
                assert!(engine.catalog.candidates.is_empty());
            }
        });
    }

    #[test]
    fn generated_probe_uses_positive_heads_and_preserves_syntax_shadowing() {
        crate::with_compiler_stack(|| {
            for generic in [false, true] {
                let store = Arc::new(crate::store::MemStore::default());
                let create = || {
                    QueryEngine::new(
                        default_environment(),
                        PhaseAdapters {
                            lexer: Some(Arc::new(crate::LexerAdapter)),
                            parser: Some(Arc::new(crate::ParserAdapter)),
                            ..Default::default()
                        },
                    )
                    .with_result_store(store.clone())
                };
                let context = default_context();
                let mut source = (0..64)
                    .map(|i| format!("fn ordinary_{i}() {{}}\n"))
                    .collect::<String>();
                if generic {
                    source.push_str("fn generated[T]() = comptime make()\n");
                }
                let mut seed = create();
                let file = seed.sources.add("probe.hir", source.clone()).unwrap();
                seed.index_source(file, &context).unwrap();
                let mut engine = create();
                let file = engine.sources.add("probe.hir", source).unwrap();
                engine.index_source(file, &context).unwrap();
                assert!(
                    engine.items.is_empty(),
                    "probe uses durable heads without materializing bodies"
                );
                assert_eq!(engine.catalog.paths().count(), 64 + usize::from(generic));
                let mut probes = 0;
                assert_eq!(
                    engine.catalog.has_instance_bodied(|_| {
                        probes += 1;
                        false
                    }),
                    generic
                );
                assert_eq!(
                    probes,
                    usize::from(generic),
                    "ordinary heads need no path lookup"
                );
                assert!(
                    !engine.catalog.has_instance_bodied(|_| true),
                    "live non-generated syntax shadows a positive stored head"
                );
                for path in engine.catalog.paths() {
                    assert_eq!(
                        engine.instance_bodied(path),
                        engine.catalog.facts(path).unwrap().instance_bodied
                    );
                }
                if generic {
                    let path = engine
                        .catalog
                        .paths()
                        .find(|path| path.0.name == "generated")
                        .unwrap()
                        .clone();
                    engine.materialize(&path).unwrap();
                    assert!(
                        engine.instance_bodied(&path),
                        "classification survives materialization"
                    );
                    assert!(!engine.catalog.has_instance_bodied(|_| false));
                }
            }
        });
    }

    #[test]
    fn parse_heads_refuse_every_single_byte_corruption() {
        crate::with_compiler_stack(|| {
            use crate::store::ResultStore;
            let store = Arc::new(crate::store::MemStore::default());
            let mut engine = QueryEngine::new(
                default_environment(),
                PhaseAdapters {
                    lexer: Some(Arc::new(crate::LexerAdapter)),
                    parser: Some(Arc::new(crate::ParserAdapter)),
                    ..Default::default()
                },
            )
            .with_result_store(store.clone());
            let context = default_context();
            let file = engine
                .sources
                .add("heads.hir", "fn value() {}\n".into())
                .unwrap();
            engine.index_source(file, &context).unwrap();
            let key = engine.catalog_key(file, &context).unwrap();
            let bytes = store.get(&key).unwrap();
            for at in 0..bytes.len() {
                let mut damaged = bytes.to_vec();
                damaged[at] ^= 1;
                let record = bincode::DefaultOptions::new()
                    .with_fixint_encoding()
                    .with_limit(damaged.len() as u64)
                    .reject_trailing_bytes()
                    .deserialize::<FileHeads>(&damaged)
                    .ok();
                assert!(
                    record.is_none_or(|record| !engine.valid_heads(file, &context, &record)),
                    "byte {at}"
                );
            }
            assert!(engine.valid_heads(file, &context, &engine.capture_heads(file, &context)));
        });
    }
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
            for mutation in 0..15 {
                let store = Arc::new(crate::store::MemStore::default());
                let mut engine = QueryEngine::new(default_environment(), PhaseAdapters::default())
                    .with_result_store(store.clone());
                let file = engine.sources.add("heads.hir", source.into()).unwrap();
                let key = engine.catalog_key(file, &context).unwrap();
                let mut record = FileHeads {
                    schema: 5,
                    integrity: ContentHash::default(),
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
                        facts: SourceFacts {
                            generator: false,
                            instance_bodied: false,
                            executable: false,
                            body: false,
                            constant: false,
                            execution: false,
                            comptime: vec![],
                            fact: false,
                            associations: vec![],
                            arms: vec![],
                        },
                        lexical: LexicalShape {
                            expressions: 0,
                            statements: 0,
                            patterns: 0,
                            types: 0,
                        },
                    }],
                };
                record.integrity = heads_integrity(&record.heads);
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
                    9 => record.heads[0].facts.associations.push((3, 0, 1)),
                    10 => record.heads[0].facts.associations.push((0, 0, u32::MAX)),
                    11 => record.heads[0].facts.arms.push(("bad".into(), 2, 1)),
                    12 => record.heads[0].facts.arms.push(("bad".into(), 0, u32::MAX)),
                    13 => record.heads[0].facts.comptime.push((u32::MAX, 0, 0)),
                    14 => {
                        record.heads[0].lexical.expressions = 1;
                        record.heads[0].facts.comptime = vec![(0, 0, 0), (0, 0, 0)];
                    }
                    _ => unreachable!(),
                }
                // Preserve coherent malicious-producer structural controls.
                record.integrity = heads_integrity(&record.heads);
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

impl QueryEngine {
    pub(crate) fn declared(&self, path: &DefPath) -> bool {
        self.items.contains_key(path) || self.catalog.body(path).is_some()
    }
    pub(crate) fn source_paths(&self) -> Vec<DefPath> {
        self.items
            .keys()
            .cloned()
            .chain(self.catalog.paths().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}
