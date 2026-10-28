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
        let owner = scalar_owner(&engine, anchor);
        assert_eq!(captured.record_owner(anchor), owner);
        if let Some((owner, occurrence)) = owner {
            assert_eq!(
                captured.record_source_anchor(&owner, occurrence),
                scalar_anchor(&engine, &owner, occurrence)
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

#[test]
fn identical_generated_sources_rebind_old_sites_but_changed_text_stays_unowned() {
    let mut engine = QueryEngine::new(crate::default_environment(), Default::default());
    let path = DefPath::core("generated");
    let logical = "<generated:core@1/generated>";
    let text = "    fn()->u8 {3u8}";
    let mut placed = |text: &str| {
        let file = engine
            .sources
            .add_logical(logical, logical, text.into())
            .unwrap();
        let anchor = engine.sources.item_anchor(file, 4);
        (file, anchor)
    };
    let (_, old) = placed(text);
    let (_, changed) = placed("    fn()->u8 {4u8}");
    let (file, current) = placed(text);
    let unrelated_file = engine
        .sources
        .add_logical("unrelated", "unrelated", text.into())
        .unwrap();
    let unrelated = engine.sources.item_anchor(unrelated_file, 4);
    engine.generated.insert(
        path,
        crate::queries::generation::GeneratedSource {
            file,
            text: text.into(),
            site: hirsel_span::Span::default(),
        },
    );
    let ty = engine.types().intern(hirsel_types::Type::unit());
    let context = NativeRecordContext::capture(&engine);
    let records = NativeRecords::new(
        engine.types().clone(),
        Arc::new(crate::store::MemStore::default()),
    );
    let row = |anchor| Row {
        origin: hirsel_span::Span::new(anchor, 0, 4).unwrap(),
        ty,
    };
    let encoded = records.encode(&context, &row(old)).unwrap();
    assert_eq!(encoded, records.encode(&context, &row(current)).unwrap());
    let decoded: Row = records.decode(&context, &encoded).unwrap();
    assert_eq!(decoded.origin.anchor, current);
    assert_eq!(context.record_owner(old), engine.record_anchor_owner(old));
    let (owner, occurrence) = context.record_owner(old).unwrap();
    assert_eq!(engine.record_anchor(&owner, occurrence), Some(current));
    assert!(context.record_owner(changed).is_none());
    assert!(records.encode(&context, &row(changed)).is_none());
    assert!(context.record_owner(unrelated).is_none());
    assert!(records.encode(&context, &row(unrelated)).is_none());
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

fn scalar_owner(engine: &QueryEngine, anchor: Anchor) -> Option<(DefPath, u32)> {
    super::super::persistent::artifact_anchor_owner(
        &engine.items,
        &engine.sources,
        &engine.generated,
        anchor,
    )
    .or_else(|| {
        engine
            .catalog
            .bindings()
            .find_map(|(site, owner)| (site == anchor).then_some(owner))
    })
}

fn scalar_anchor(engine: &QueryEngine, path: &DefPath, occurrence: u32) -> Option<Anchor> {
    engine
        .items
        .get(path)
        .and_then(|items| items.iter().find(|item| item.occurrence == occurrence))
        .map(|item| item.anchor)
        .or_else(|| engine.catalog.anchor(path, occurrence))
        .or_else(|| {
            let (_, generated) = engine.generated.iter().find(|(owner, source)| {
                super::super::persistent::generated_codec_owner(owner, &source.text) == *path
            })?;
            engine.sources.anchors().find_map(|(anchor, file, base)| {
                (file == generated.file && base == occurrence).then_some(anchor)
            })
        })
        .or_else(|| {
            engine.sources.anchors().find_map(|(anchor, _, _)| {
                (scalar_owner(engine, anchor) == Some((path.clone(), occurrence))).then_some(anchor)
            })
        })
}

fn assert_scalar_bindings(engine: &QueryEngine) {
    let captured = NativeRecordContext::capture(engine);
    for (anchor, _, _) in engine.sources.anchors() {
        let owner = scalar_owner(engine, anchor);
        assert_eq!(captured.record_owner(anchor), owner);
        if let Some((path, occurrence)) = owner {
            assert_eq!(
                captured.record_source_anchor(&path, occurrence),
                scalar_anchor(engine, &path, occurrence)
            );
        }
    }
    for (path, items) in &engine.items {
        for item in items {
            assert_eq!(
                captured.record_source_anchor(path, item.occurrence),
                scalar_anchor(engine, path, item.occurrence)
            );
        }
    }
    assert!(captured.record_owner(Anchor::fresh()).is_none());
    assert!(
        captured
            .record_source_anchor(&DefPath::core("foreign-owner"), 0)
            .is_none()
    );
}

#[test]
fn ownership_product_tracks_removal_reordering_relocation_and_parse_eviction() {
    let mut engine = QueryEngine::new(crate::default_environment(), Default::default());
    let context = crate::default_context();
    let index = |engine: &mut QueryEngine, name: &str, text: &str| {
        let file = engine.sources.add(name, text.into()).unwrap();
        let tokens = hirsel_lexer::lex(file, &engine.sources).tokens;
        let mut parsed = hirsel_parser::parse(file, tokens, &context, &mut engine.sources);
        assert!(parsed.diagnostics.is_empty());
        engine.index(&mut parsed.syntax, &context).unwrap();
        engine.parsed.clear();
        assert_scalar_bindings(engine);
    };
    index(
        &mut engine,
        "a.hir",
        "fn same()->u8{1u8}\nfn same()->u8{2u8}\nfn gone()->u8{3u8}",
    );
    let old = engine.source_records();
    let gone = engine
        .items
        .iter()
        .find(|(p, _)| p.0.name == "gone")
        .unwrap()
        .1[0]
        .anchor;
    index(
        &mut engine,
        "a.hir",
        "fn same()->u8{2u8}\nfn same()->u8{1u8}",
    );
    assert!(engine.record_anchor_owner(gone).is_none());
    assert!(old.record_owner(gone).is_some());
    index(&mut engine, "a.hir", "");
    index(
        &mut engine,
        "b.hir",
        "fn same()->u8{2u8}\nfn same()->u8{1u8}",
    );
    let first = engine.source_records();
    let second = engine.source_records();
    assert!(Arc::ptr_eq(&first.bindings, &second.bindings));
    let bindings = Arc::downgrade(&first.bindings);
    drop(first);
    drop(second);
    index(&mut engine, "b.hir", "fn same()->u8{4u8}");
    assert!(
        bindings.upgrade().is_none(),
        "retired products are released after their last worker"
    );
    assert!(old.record_owner(gone).is_some());
}

#[test]
fn ownership_projection_is_charged_and_generated_changes_retire_both_directions() {
    let mut engine = QueryEngine::new(crate::default_environment(), Default::default());
    for name in ["a", "b", "c"] {
        let file = engine.sources.add(name, "fn()->u8{1u8}".into()).unwrap();
        engine.generated.insert(
            DefPath::core(name),
            crate::queries::generation::GeneratedSource {
                file,
                text: "fn()->u8{1u8}".into(),
                site: Default::default(),
            },
        );
    }
    assert_scalar_bindings(&engine);
    let old = engine.source_records();
    let file = engine.sources.add("extra", "fn()->u8{2u8}".into()).unwrap();
    let anchor = engine.sources.item_anchor(file, 0);
    assert!(engine.record_anchor_owner(anchor).is_none());
    engine.generated.insert(
        DefPath::core("a"),
        crate::queries::generation::GeneratedSource {
            file,
            text: "fn()->u8{2u8}".into(),
            site: Default::default(),
        },
    );
    assert_scalar_bindings(&engine);
    let current = engine.source_records();
    for (owner, occurrence) in old
        .bindings
        .owners
        .values()
        .filter(|(owner, _)| owner.0.name.starts_with("$generated-source:a:"))
    {
        assert!(current.record_source_anchor(owner, *occurrence).is_none());
    }
    let bound = current.input_bytes().unwrap();
    assert!(bound > size_of::<NativeRecordContext>() as u64);
    engine.generated.clear();
    let (next, allocated) = crate::test_alloc::measured_independent(|| engine.source_records());
    assert!(allocated.live as u64 <= next.input_bytes().unwrap());
    assert!(next.bindings.owners.is_empty());
    assert!(current.record_owner(anchor).is_some());
}

#[test]
fn ownership_charge_covers_populated_maps_and_releases_the_last_generation() {
    let mut engine = QueryEngine::new(crate::default_environment(), Default::default());
    let text = (0..64)
        .map(|i| format!("fn owner_with_a_long_name_{i}()->u64{{{i}u64}}\n"))
        .collect();
    let file = engine.sources.add("charged.hir", text).unwrap();
    let tokens = hirsel_lexer::lex(file, &engine.sources).tokens;
    let context = crate::default_context();
    let mut parsed = hirsel_parser::parse(file, tokens, &context, &mut engine.sources);
    assert!(parsed.diagnostics.is_empty());
    engine.index(&mut parsed.syntax, &context).unwrap();
    for path in engine.items.keys() {
        DefId::intern(path);
    }
    let (owners, allocated) = crate::test_alloc::measured_independent(|| engine.source_records());
    assert_eq!(owners.bindings.owners.len(), 64);
    assert!(allocated.live > 0);
    assert!(allocated.live as u64 <= owners.input_bytes().unwrap());
    let old = Arc::downgrade(&owners.bindings);
    engine.items.clear();
    assert!(engine.source_records().bindings.owners.is_empty());
    assert!(old.upgrade().is_some());
    drop(owners);
    assert!(old.upgrade().is_none());
}

#[test]
fn a_foreign_source_map_cannot_restore_unplaced_declaration_anchors() {
    let mut engine = QueryEngine::new(crate::default_environment(), Default::default());
    let file = engine
        .sources
        .add("current.hir", "pub fn main()->u64{7u64}".into())
        .unwrap();
    let tokens = hirsel_lexer::lex(file, &engine.sources).tokens;
    let context = crate::default_context();
    let mut parsed = hirsel_parser::parse(file, tokens, &context, &mut engine.sources);
    assert!(parsed.diagnostics.is_empty());
    engine.index(&mut parsed.syntax, &context).unwrap();
    let syntax = &parsed.syntax.items[0];
    let path = syntax.item.definition.clone();
    let anchor = syntax.anchor;
    let records = NativeRecords::new(
        engine.types().clone(),
        Arc::new(crate::store::MemStore::default()),
    );
    let ty = engine.types().intern(hirsel_types::Type::unit());
    let bytes = records
        .encode(
            &engine,
            &Row {
                origin: hirsel_span::Span::new(anchor, 0, 4).unwrap(),
                ty,
            },
        )
        .unwrap();
    let mut foreign = hirsel_span::SourceMap::default();
    foreign
        .add("foreign.hir", "pub fn other()->u64{9u64}".into())
        .unwrap();
    engine.sources = foreign;
    let current = engine.source_records();
    assert!(current.record_owner(anchor).is_none());
    assert!(current.record_source_anchor(&path, 0).is_none());
    assert!(records.decode::<Row>(&current, &bytes).is_none());
}
