//! The ordinary Parse output, demanded independently of declaration heads.
//! Uses serde/bincode and the same source-anchor codec as all other products;
//! it contains no type table, semantic acceptance or verifier authority.
use super::*;
use hirsel_span::{Anchor, AnchorCodec, Span};

#[derive(Serialize, Deserialize)]
struct ParsedSource {
    schema: u32,
    context: FileContext,
    source: ContentHash,
    bases: Vec<u32>,
    payload: Vec<u8>,
}
fn options() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(hirsel_artifact_store::MAX_RECORD as u64)
        .reject_trailing_bytes()
}
fn bounds(text: &str, bases: &[u32], span: Span) -> Option<(usize, usize)> {
    let base = *bases.get(span.anchor.0.checked_sub(1)? as usize)?;
    let start = base.checked_add(span.start)? as usize;
    let end = base.checked_add(span.end)? as usize;
    text.get(start..end)?;
    Some((start, end))
}
fn valid_output(
    output: &QueryOutput<SyntaxFile>,
    context: &FileContext,
    text: &str,
    bases: &[u32],
) -> bool {
    let valid_span = |span| bounds(text, bases, span).is_some();
    if output.value.has_syntax_errors
        || !output.dependencies.is_empty()
        || has_errors(&output.diagnostics)
    {
        return false;
    }
    for diagnostic in &output.diagnostics {
        if !valid_span(diagnostic.primary)
            || diagnostic.secondary.iter().any(|s| !valid_span(s.span))
            || diagnostic.fix.as_ref().is_some_and(|s| !valid_span(s.span))
            || diagnostic.obligations.iter().any(|o| match o {
                hirsel_diag::Obligation::GenericBound { arguments } => {
                    arguments.iter().any(|s| !valid_span(*s))
                }
                hirsel_diag::Obligation::ParameterMode { argument } => !valid_span(*argument),
                _ => false,
            })
        {
            return false;
        }
    }
    let mut cursor = 0;
    for token in output.value.tape() {
        let Some((start, end)) = bounds(text, bases, token.span) else {
            return false;
        };
        if start != cursor {
            return false;
        }
        cursor = end;
    }
    if cursor != text.len() {
        return false;
    }
    output.value.items.iter().all(|syntax| {
        let path = &syntax.item.definition;
        if path.0.package != context.package
            || path.0.modules != context.modules
            || !syntax.structurally_valid(valid_span)
        {
            return false;
        }
        let Some(first) = syntax.tree.tokens.first() else {
            return false;
        };
        let Some(last) = syntax.tree.tokens.last() else {
            return false;
        };
        let Some((start, _)) = bounds(text, bases, first.span) else {
            return false;
        };
        let Some((_, end)) = bounds(text, bases, last.span) else {
            return false;
        };
        text.get(start..end)
            .is_some_and(|bytes| ContentHash::of_bytes(bytes.as_bytes()) == syntax.text_hash)
    })
}
impl QueryEngine {
    fn parse_syntax_key(
        &self,
        file: FileId,
        context: &FileContext,
    ) -> Option<crate::store::EntryKey> {
        let mut key = self.catalog_key(file, context)?;
        key.0 = ContentHash::of_parts("hirsel.parse-syntax.address.v1", [key.0.0.as_slice()]);
        Some(key)
    }
    fn encode_parse_syntax(
        &self,
        file: FileId,
        context: &FileContext,
        output: &QueryOutput<SyntaxFile>,
    ) -> Option<Vec<u8>> {
        if output.value.has_syntax_errors || has_errors(&output.diagnostics) {
            return None;
        }
        let source = self.sources.file(file)?;
        let mut anchors = AnchorCodec::Collect(vec![]);
        let payload =
            hirsel_span::with_anchor_codec(&mut anchors, || options().serialize(output)).ok()?;
        let AnchorCodec::Collect(anchors) = anchors else {
            unreachable!()
        };
        let bases = anchors
            .into_iter()
            .map(|anchor| {
                let (owner, base) = self.sources.anchor(anchor)?;
                (owner == file).then_some(base)
            })
            .collect::<Option<Vec<_>>>()?;
        options()
            .serialize(&ParsedSource {
                schema: 1,
                context: context.clone(),
                source: ContentHash::of_bytes(source.text().as_bytes()),
                bases,
                payload,
            })
            .ok()
    }
    pub(super) fn load_parse_syntax(
        &self,
        file: FileId,
        context: &FileContext,
    ) -> Option<QueryOutput<SyntaxFile>> {
        let key = self.parse_syntax_key(file, context)?;
        let bytes = self.result_store.as_ref()?.get(&key)?;
        let record: ParsedSource = options().deserialize(&bytes).ok()?;
        let source = self.sources.file(file)?;
        if record.schema != 1
            || record.context != *context
            || record.source != ContentHash::of_bytes(source.text().as_bytes())
            || record
                .bases
                .iter()
                .any(|&base| !source.text().is_char_boundary(base as usize))
        {
            return None;
        }
        // First decode only numbered, unplaced anchors. No malformed product
        // can place source owners or reach an arena-indexing consumer.
        let numbered = (1..=u32::try_from(record.bases.len()).ok()?)
            .map(Anchor)
            .collect();
        let output: QueryOutput<SyntaxFile> =
            hirsel_span::with_anchor_codec(&mut AnchorCodec::Decode(numbered), || {
                options().deserialize(&record.payload)
            })
            .ok()?;
        if !valid_output(&output, context, source.text(), &record.bases) {
            return None;
        }
        let mut anchors = vec![None; record.bases.len()];
        for syntax in &output.value.items {
            let slot = syntax.anchor.0.checked_sub(1)? as usize;
            let base = *record.bases.get(slot)?;
            let path = &syntax.item.definition;
            let anchor = self
                .catalog
                .candidates
                .get(path)
                .into_iter()
                .flatten()
                .find(|candidate| candidate.file == file && candidate.base == base)
                .map(|candidate| candidate.head.name.anchor)
                .or_else(|| {
                    self.items
                        .get(path)?
                        .iter()
                        .find(|item| self.sources.anchor(item.anchor) == Some((file, base)))
                        .map(|item| item.anchor)
                })?;
            if anchors[slot].replace(anchor).is_some() {
                return None;
            }
        }
        for (slot, anchor) in anchors.iter_mut().enumerate() {
            if anchor.is_none() {
                if record.bases[slot] != 0 {
                    return None;
                }
                *anchor = Some(self.sources.file_anchor(file));
            }
        }
        // Release the bounded validation tree before materializing placed AST
        // owners. At most one source-file tree is retained by this operation.
        drop(output);
        let anchors = anchors.into_iter().collect::<Option<Vec<_>>>()?;
        let mut output: QueryOutput<SyntaxFile> =
            hirsel_span::with_anchor_codec(&mut AnchorCodec::Decode(anchors), || {
                options().deserialize(&record.payload)
            })
            .ok()?;
        output.value.file = file;
        Some(output)
    }
    pub(in crate::queries) fn store_parse_syntax(
        &self,
        file: FileId,
        context: &FileContext,
        output: &QueryOutput<SyntaxFile>,
    ) -> Result<(), QueryError> {
        let Some(key) = self.parse_syntax_key(file, context) else {
            return Ok(());
        };
        let Some(bytes) = self.encode_parse_syntax(file, context, output) else {
            return Ok(());
        };
        if self.verify_store == crate::store::VerifyStore::Recompute
            && let Some(previous) = self.load_parse_syntax(file, context)
            && self.encode_parse_syntax(file, context, &previous).as_ref() != Some(&bytes)
        {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "store verification".into(),
                message: "parse syntax differs from recomputation".into(),
            }));
        }
        self.result_store.as_ref().unwrap().put(&key, &bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{MemStore, ResultStore, VerifyStore};

    const SOURCE: &str = "fn value() -> u8 { 1u8 }\n";
    fn engine(store: &Arc<MemStore>) -> QueryEngine {
        QueryEngine::new(
            default_environment(),
            PhaseAdapters {
                lexer: Some(Arc::new(crate::LexerAdapter)),
                parser: Some(Arc::new(crate::ParserAdapter)),
                ..Default::default()
            },
        )
        .with_result_store(store.clone())
    }
    fn indexed(store: &Arc<MemStore>, context: &FileContext) -> (QueryEngine, FileId, DefPath) {
        let mut engine = engine(store);
        let file = engine.sources.add("syntax.hir", SOURCE.into()).unwrap();
        engine.index_source(file, context).unwrap();
        let path = DefPath(ItemPath {
            package: context.package.clone(),
            modules: context.modules.clone(),
            name: "value".into(),
        });
        (engine, file, path)
    }
    fn mutate(
        engine: &QueryEngine,
        file: FileId,
        context: &FileContext,
        store: &MemStore,
        change: impl FnOnce(&mut QueryOutput<SyntaxFile>),
    ) {
        let key = engine.parse_syntax_key(file, context).unwrap();
        let bytes = store.get(&key).unwrap();
        let mut record: ParsedSource = options().deserialize(&bytes).unwrap();
        let numbered = || (1..=record.bases.len() as u32).map(Anchor).collect();
        let mut output: QueryOutput<SyntaxFile> =
            hirsel_span::with_anchor_codec(&mut AnchorCodec::Decode(numbered()), || {
                options().deserialize(&record.payload)
            })
            .unwrap();
        change(&mut output);
        // Preserve the record's numbered slots, even when malicious syntax
        // changes the order of first occurrence of an anchor.
        record.payload =
            hirsel_span::with_anchor_codec(&mut AnchorCodec::Collect(numbered()), || {
                options().serialize(&output)
            })
            .unwrap();
        store.put(&key, &options().serialize(&record).unwrap());
    }
    #[test]
    fn parse_cache_restores_real_library_syntax() {
        crate::with_compiler_stack(|| {
            for (name, source) in [
                (
                    "two-items",
                    "fn first() -> u8 {1u8}\nfn second() -> u8 {2u8}\n",
                ),
                (
                    "u64",
                    include_str!("../../../../../stdlib/std@1/u64_ops.hir"),
                ),
                (
                    "string",
                    include_str!("../../../../../stdlib/std@1/string.hir"),
                ),
                (
                    "actor",
                    include_str!("../../../../../stdlib/std@1/actor.hir"),
                ),
                ("list", include_str!("../../../../../stdlib/std@1/list.hir")),
                (
                    "derive",
                    include_str!("../../../../../stdlib/std@1/derive.hir"),
                ),
            ] {
                let store = Arc::new(MemStore::default());
                let context = default_context();
                let mut cold = engine(&store);
                let file = cold
                    .sources
                    .add(format!("{name}.hir"), source.into())
                    .unwrap();
                cold.index_source(file, &context).unwrap();
                let key = cold.parse_syntax_key(file, &context).unwrap();
                assert!(store.get(&key).is_some(), "{name}: missing record");
                let output = &cold.parsed[&file].2.as_ref().unwrap();
                for syntax in &output.value.items {
                    assert!(
                        syntax.structurally_valid(|span| {
                            let absolute = cold.sources.resolve(span);
                            absolute.file == file
                                && source
                                    .get(absolute.start as usize..absolute.end as usize)
                                    .is_some()
                        }),
                        "{name}: invalid structure {}",
                        syntax.item.definition
                    );
                }
                assert!(
                    cold.load_parse_syntax(file, &context).is_some(),
                    "{name}: refused cold output"
                );
                let mut warm = engine(&store);
                let file = warm
                    .sources
                    .add(format!("moved/{name}.hir"), source.into())
                    .unwrap();
                warm.index_source(file, &context).unwrap();
                warm.adapters.parser = None;
                warm.materialize_source(file).unwrap();
            }
        });
    }
    #[test]
    fn parse_cache_rebinds_anchors_and_recomputes_duplicate_occurrences() {
        crate::with_compiler_stack(|| {
            let store = Arc::new(MemStore::default());
            let context = default_context();
            let (cold, file, path) = indexed(&store, &context);
            let original = cold.items[&path][0].anchor;
            assert!(cold.load_parse_syntax(file, &context).is_some());
            let mut warm = engine(&store);
            // An earlier duplicate changes the occurrence of the cached owner.
            let earlier = warm.sources.add("a.hir", SOURCE.into()).unwrap();
            warm.index_source(earlier, &context).unwrap();
            let moved = warm.sources.add("moved/syntax.hir", SOURCE.into()).unwrap();
            warm.index_source(moved, &context).unwrap();
            assert!(warm.catalog.files.contains_key(&moved));
            let placed = warm.catalog.candidates[&path][0].head.name.anchor;
            assert_ne!(original, placed);
            warm.adapters.parser = None;
            warm.materialize_source(moved).unwrap();
            let restored = warm.items[&path]
                .iter()
                .find(|s| s.anchor == placed)
                .unwrap();
            assert_eq!(restored.occurrence, 1);
            assert_eq!(warm.sources.anchor(restored.anchor).unwrap().0, moved);
            assert!(restored.structurally_valid(|s| warm.sources.resolve(s).file == moved));
            assert!(warm.signatures.is_empty());
            assert!(warm.lowered.is_empty());
            assert!(warm.lower_receipts.is_empty());
        });
    }
    #[test]
    fn parse_cache_binds_source_package_and_module() {
        crate::with_compiler_stack(|| {
            let store = Arc::new(MemStore::default());
            let context = default_context();
            let (cold, file, _) = indexed(&store, &context);
            let bytes = store
                .get(&cold.parse_syntax_key(file, &context).unwrap())
                .unwrap();
            for change in 0..3 {
                let mut context = context.clone();
                let source = match change {
                    0 => {
                        context.package.version = "other".into();
                        SOURCE
                    }
                    1 => {
                        context.modules.push("other".into());
                        SOURCE
                    }
                    _ => "fn value() -> u8 { 2u8 }\n",
                };
                let mut reader = engine(&store);
                let file = reader.sources.add("syntax.hir", source.into()).unwrap();
                let key = reader.parse_syntax_key(file, &context).unwrap();
                store.put(&key, &bytes);
                assert!(reader.load_parse_syntax(file, &context).is_none());
                // The normal miss still parses under the actual context.
                reader.index_source(file, &context).unwrap();
                assert!(
                    reader
                        .items
                        .keys()
                        .all(|p| p.0.package == context.package && p.0.modules == context.modules)
                );
            }
        });
    }
    #[test]
    fn parse_cache_refuses_malformed_payload_spans_tokens_and_arenas() {
        crate::with_compiler_stack(|| {
            for change in 0..8 {
                let store = Arc::new(MemStore::default());
                let context = default_context();
                let (cold, file, _) = indexed(&store, &context);
                mutate(&cold, file, &context, &store, |output| {
                    let syntax = Arc::make_mut(&mut output.value.items[0]);
                    match change {
                        0 => syntax.item.name_span.end = u32::MAX,
                        1 => syntax.tree.tokens[0].span.start = 1,
                        2 => syntax.tree.expressions[0].origin.tokens.end = u32::MAX,
                        3 => {
                            syntax.tree.expressions[0].kind =
                                hirsel_syntax::ExprKind::Tuple(vec![hirsel_syntax::ExprId(
                                    u32::MAX,
                                )])
                        }
                        4 => {
                            syntax.tree.expressions[0].kind =
                                hirsel_syntax::ExprKind::Tuple(vec![hirsel_syntax::ExprId(0)])
                        }
                        5 => syntax.item.definition.0.modules.push("forged".into()),
                        6 => output.value.has_syntax_errors = true,
                        _ => syntax.text_hash = ContentHash::default(),
                    }
                });
                assert!(
                    cold.load_parse_syntax(file, &context).is_none(),
                    "mutation {change}"
                );
                let key = cold.parse_syntax_key(file, &context).unwrap();
                store.put(&key, &[255, 0, 1]);
                assert!(cold.load_parse_syntax(file, &context).is_none());
            }
        });
    }
    #[test]
    fn parse_cache_verify_store_recomputes_and_detects_canonical_mismatch() {
        crate::with_compiler_stack(|| {
            let context = default_context();
            for changed in [false, true] {
                let store = Arc::new(MemStore::default());
                let (cold, file, _) = indexed(&store, &context);
                if changed {
                    mutate(&cold, file, &context, &store, |output| {
                        // Structurally safe but different syntax is still not
                        // accepted as the recomputed parser result.
                        Arc::make_mut(&mut output.value.items[0]).tree.expressions[0].kind =
                            hirsel_syntax::ExprKind::Tuple(vec![]);
                    });
                    assert!(cold.load_parse_syntax(file, &context).is_some());
                }
                let mut reader = engine(&store).with_verify_store(VerifyStore::Recompute);
                let file = reader.sources.add("syntax.hir", SOURCE.into()).unwrap();
                let result = reader.index_source(file, &context);
                if changed {
                    assert!(
                        matches!(result, Err(QueryError::Operational(ref failure)) if failure.message == "parse syntax differs from recomputation")
                    );
                } else {
                    result.unwrap();
                    assert!(!reader.parsed.is_empty());
                }
                assert!(reader.signatures.is_empty());
                assert!(reader.lowered.is_empty());
                assert!(reader.lower_receipts.is_empty());
            }
        });
    }
}
