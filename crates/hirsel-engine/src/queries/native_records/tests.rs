use super::*;

#[test]
fn captured_anchor_bindings_match_scalar_selection_with_duplicate_and_generated_owners() {
    let mut engine = QueryEngine::new(crate::default_environment(), Default::default());
    let file = engine
        .sources
        .add(
            "duplicates.hir",
            "fn same()->u8{1u8}\nfn same()->u8{2u8}".into(),
        )
        .unwrap();
    let tokens = hirsel_lexer::lex(file, &engine.sources).tokens;
    let context = crate::default_context();
    let mut parsed = hirsel_parser::parse(file, tokens, &context, &mut engine.sources);
    assert!(parsed.diagnostics.is_empty());
    engine.index(&mut parsed.syntax, &context).unwrap();
    let mut alias = (*engine.items.values().next().unwrap()[0]).clone();
    alias.item.definition.0.name = "aaa_alias".into();
    engine
        .items
        .insert(alias.item.definition.clone(), vec![Arc::new(alias)]);
    let file = engine
        .sources
        .add("generated.hir", "    fn()->u8 {3u8}".into())
        .unwrap();
    engine.sources.item_anchor(file, 4);
    engine.sources.item_anchor(file, 9);
    for name in ["generated", "zzz_generated"] {
        engine.generated.insert(
            DefPath::core(name),
            crate::queries::generation::GeneratedSource {
                file,
                text: "    fn()->u8 {3u8}".into(),
                site: hirsel_span::Span::default(),
            },
        );
    }
    engine.sources.add("unowned.hir", "".into()).unwrap();
    let captured = NativeRecordContext::capture(&engine);
    for (anchor, _, _) in engine.sources.anchors() {
        let owner = engine.record_anchor_owner(anchor);
        assert_eq!(captured.record_owner(anchor), owner);
        if let Some((owner, occurrence)) = owner {
            assert_eq!(
                captured.record_source_anchor(&owner, occurrence),
                engine.record_anchor(&owner, occurrence)
            );
        }
    }
}

fn context(name: &str, text: &str) -> (NativeRecordContext, Anchor, hirsel_types::TypeId) {
    let mut engine = QueryEngine::new(crate::default_environment(), Default::default());
    let file = engine.sources.add(name, text.into()).unwrap();
    let tokens = hirsel_lexer::lex(file, &engine.sources).tokens;
    let context = crate::default_context();
    let mut parsed = hirsel_parser::parse(file, tokens, &context, &mut engine.sources);
    assert!(parsed.diagnostics.is_empty());
    engine.index(&mut parsed.syntax, &context).unwrap();
    let anchor = engine
        .sources
        .anchors()
        .find_map(|(anchor, _, _)| {
            (engine.sources.anchor(anchor).unwrap().0 == file)
                .then(|| engine.record_anchor_owner(anchor).map(|_| anchor))
                .flatten()
        })
        .unwrap();
    let ty = engine.types().intern(hirsel_types::Type::unit());
    (NativeRecordContext::capture(&engine), anchor, ty)
}
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Row {
    origin: hirsel_span::Span,
    ty: hirsel_types::TypeId,
}
#[test]
fn source_records_rebind_real_owners_after_engine_release_and_refuse_another_context() {
    let store = Arc::new(crate::store::MemStore::default());
    let text = "pub fn main()->u64 {7u64}";
    let (source, anchor, ty) = context("first/records.hir", text);
    let records = NativeRecords::new(source.record_types().clone(), store.clone());
    let bytes = records
        .encode(
            &source,
            &Row {
                origin: hirsel_span::Span::new(anchor, 0, 4).unwrap(),
                ty,
            },
        )
        .unwrap();
    let original: Row = records.decode(&source, &bytes).unwrap();
    assert_eq!(original.origin.anchor, anchor);
    assert_eq!((original.origin.start, original.origin.end), (0, 4));
    assert_eq!(original.ty, ty);
    let (relocated, other_anchor, _) = context("second/records.hir", text);
    assert_ne!(anchor, other_anchor);
    let other = NativeRecords::new(relocated.record_types().clone(), store.clone());
    let rebound: Row = other.decode(&relocated, &bytes).unwrap();
    assert_eq!(rebound.origin.anchor, other_anchor);
    assert_eq!((rebound.origin.start, rebound.origin.end), (0, 4));
    assert_eq!(other.encode(&relocated, &rebound).unwrap(), bytes);
    let (unrelated, _, _) = context("other/records.hir", "pub fn other()->u64 {3u64}");
    let unknown = NativeRecords::new(unrelated.record_types().clone(), store.clone());
    assert!(unknown.decode::<Row>(&unrelated, &bytes).is_none());
    let wrong = NativeRecords::new(hirsel_types::TypeInterner::new(None), store);
    assert!(
        wrong
            .encode(
                &source,
                &Row {
                    origin: hirsel_span::Span::new(anchor, 0, 4).unwrap(),
                    ty
                }
            )
            .is_none()
    );
    assert!(wrong.decode::<Row>(&source, &bytes).is_none());
}

#[test]
fn generated_source_owner_is_independent_of_codec_identity_and_selector_position() {
    fn generated(text: &str) -> (NativeRecordContext, DefPath, Anchor, hirsel_types::TypeId) {
        let mut engine = QueryEngine::new(crate::default_environment(), Default::default());
        let path = DefPath::core("generated");
        let file = engine.sources.add("generated.hir", text.into()).unwrap();
        let anchor = engine.sources.item_anchor(file, 4);
        engine.generated.insert(
            path.clone(),
            crate::queries::generation::GeneratedSource {
                file,
                text: text.into(),
                site: hirsel_span::Span::default(),
            },
        );
        let ty = engine.types().intern(hirsel_types::Type::unit());
        (NativeRecordContext::capture(&engine), path, anchor, ty)
    }
    let (context, path, anchor, ty) = generated("    fn(x:u64)->u64 {9u64/x}");
    assert_eq!(context.source_definition(anchor), Some(&path));
    let (codec_owner, occurrence) = context.record_owner(anchor).unwrap();
    assert_ne!(codec_owner, path);
    assert!(
        codec_owner
            .0
            .name
            .starts_with("$generated-source:generated:")
    );
    assert_eq!(occurrence, 4);
    assert_eq!(
        context.record_source_anchor(&codec_owner, occurrence),
        Some(anchor)
    );
    assert_eq!(context.record_source_anchor(&path, 0), None);
    assert_eq!(context.source_definition(Anchor(u32::MAX)), None);
    let store = Arc::new(crate::store::MemStore::default());
    let records = NativeRecords::new(context.record_types().clone(), store.clone());
    let bytes = records
        .encode(
            &context,
            &Row {
                origin: hirsel_span::Span::new(anchor, 0, 2).unwrap(),
                ty,
            },
        )
        .unwrap();
    let same = records.decode::<Row>(&context, &bytes).unwrap();
    assert_eq!(same.origin.anchor, anchor);
    let (changed, changed_path, changed_anchor, _) = generated("    fn(x:u64)->u64 {8u64/x}");
    assert_eq!(changed_path, path);
    assert_eq!(changed.source_definition(changed_anchor), Some(&path));
    assert_ne!(
        changed.record_owner(changed_anchor),
        Some((codec_owner, occurrence))
    );
    let other = NativeRecords::new(changed.record_types().clone(), store);
    assert!(
        other.decode::<Row>(&changed, &bytes).is_none(),
        "a declaration association cannot admit a different generated text"
    );
}
