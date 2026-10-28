use super::*;
use crate::{DefinitionSummary, GenericParameterRef, ParameterGrowth, QueryEngine, ReadsBuilder};
use hirsel_syntax::LexicalOwner;
use hirsel_types::{NameSite, TypeParamId, TypeParameterRef};

fn parameter(id: DefId) -> (DefId, GenericParameterRef) {
    (
        id,
        GenericParameterRef::Type(TypeParameterRef::body(
            LexicalOwner::first(id.path()).site(NameSite::Item),
            TypeParamId(0),
        )),
    )
}

fn context() -> (QueryEngine, NativeRecordContext, Vec<DefId>, Vec<Span>) {
    let mut engine = QueryEngine::new(crate::default_environment(), Default::default());
    let file = engine
        .sources
        .add("components.hir", "fn a(){}\nfn b(){}\nfn c(){}\n".into())
        .unwrap();
    let tokens = hirsel_lexer::lex(file, &engine.sources).tokens;
    let context = crate::default_context();
    let parsed = hirsel_parser::parse(file, tokens, &context, &mut engine.sources);
    assert!(parsed.diagnostics.is_empty());
    for syntax in parsed.syntax.items {
        engine
            .items
            .entry(syntax.item.definition.clone())
            .or_default()
            .push(syntax);
    }
    let ids = engine.items.keys().map(DefId::intern).collect();
    let sites = engine
        .items
        .values()
        .map(|items| items[0].item.origin.span)
        .collect();
    let source = NativeRecordContext::capture(&engine);
    (engine, source, ids, sites)
}

fn summary(owner: DefId, edges: Vec<crate::GrowthEdge>) -> Arc<CheckedSummary> {
    Arc::new(CheckedSummary {
        summary: DefinitionSummary {
            owner,
            frames: Box::new([]),
            calls: Box::new([]),
            staged: Box::new([]),
        },
        growth: ParameterGrowth {
            edges: edges.into(),
        },
    })
}

#[test]
fn equal_settled_component_stops_changed_member_and_preserves_unrelated_outputs() {
    let (engine, source, ids, sites) = context();
    let store = crate::store::MemStore::default();
    let records = NativeRecords::new(
        engine.types().clone(),
        Arc::new(crate::store::MemStore::default()),
    );
    let producer = SourceGrowthProducer {
        records: &records,
        source: &source,
        store: &store,
        namespace: Address(ContentHash::of_bytes(b"source components")),
        scratch: 8 << 20,
    };
    let reads = ReadsBuilder::new().finish().unwrap();
    let selected = FACT.reference(b"accepted checked output").unwrap();
    let make = |extra, grows| {
        let mut edges = vec![(parameter(ids[0]), parameter(ids[1]), grows, sites[0])];
        if extra {
            edges.push((parameter(ids[0]), parameter(ids[0]), false, sites[0]));
        }
        vec![
            producer
                .fact(
                    Address(ids[0].path().fingerprint()),
                    summary(ids[0], edges),
                    &reads,
                    selected,
                )
                .unwrap(),
            producer
                .fact(
                    Address(ids[1].path().fingerprint()),
                    summary(
                        ids[1],
                        vec![(parameter(ids[1]), parameter(ids[0]), false, sites[1])],
                    ),
                    &reads,
                    selected,
                )
                .unwrap(),
            producer
                .fact(
                    Address(ids[2].path().fingerprint()),
                    summary(
                        ids[2],
                        vec![(parameter(ids[2]), parameter(ids[2]), false, sites[2])],
                    ),
                    &reads,
                    selected,
                )
                .unwrap(),
        ]
    };
    let first = producer.settle(&make(false, false)).unwrap();
    let changed = producer.settle(&make(true, false)).unwrap();
    assert_eq!(first.membership.record, changed.membership.record);
    assert_ne!(
        first.components[0].product.action,
        changed.components[0].product.action
    );
    assert_eq!(
        first.components[0].product.record, changed.components[0].product.record,
        "the settled verdict is the exact consumer cutoff even when its action changed"
    );
    assert_eq!(first.components[1].product, changed.components[1].product);
    let mut graph = crate::memo::MemoGraph::default();
    let key = crate::memo::MemoKey::new(crate::QueryKind::Summary, ids[0]);
    let original = graph.insert_output(
        key,
        first.components[0].product.action.0,
        Ok(Arc::new(())),
        Arc::new(reads.clone()),
        crate::memo::Durability::Low,
        Some(first.components[0].product.record.output.0),
    );
    let mut edits = crate::memo::ChangeSet::default();
    edits.text(ids[0], crate::memo::Durability::Low);
    graph.apply(&edits);
    let equal = graph.insert_output(
        key,
        changed.components[0].product.action.0,
        Ok(Arc::new(())),
        Arc::new(reads.clone()),
        crate::memo::Durability::Low,
        Some(changed.components[0].product.record.output.0),
    );
    assert_eq!(
        original.changed_at, equal.changed_at,
        "an actual settled facet backdates across the changed member"
    );
    let rejected = producer.settle(&make(true, true)).unwrap();
    assert_ne!(
        changed.components[0].product.record,
        rejected.components[0].product.record
    );
    assert_eq!(&*rejected.components[0].growing, &[sites[0]]);
    let mut reordered = make(true, true);
    reordered.reverse();
    let reordered = producer.settle(&reordered).unwrap();
    assert_eq!(reordered.membership, rejected.membership);
    for (a, b) in reordered.components.iter().zip(&rejected.components) {
        assert_eq!(a.product, b.product);
    }
}

#[test]
fn changed_observations_are_republished_before_equal_fact_cutoff() {
    let (engine, source, ids, _) = context();
    let store = crate::store::MemStore::default();
    let records = NativeRecords::new(
        engine.types().clone(),
        Arc::new(crate::store::MemStore::default()),
    );
    let producer = SourceGrowthProducer {
        records: &records,
        source: &source,
        store: &store,
        namespace: Address(ContentHash::of_bytes(b"observations")),
        scratch: 8 << 20,
    };
    let selected = FACT.reference(b"accepted checked output").unwrap();
    for key in [
        crate::ReadKey::PrivateSummary(ids[0]),
        crate::ReadKey::Lookup(crate::memo::LookupKey::of(
            crate::memo::LookupSource::Provider,
            &crate::default_context(),
            None,
            &["missing".into()],
        )),
        crate::ReadKey::Lookup(crate::memo::LookupKey::of(
            crate::memo::LookupSource::Manifest,
            &crate::default_context(),
            None,
            &["module".into()],
        )),
    ] {
        let make = |value| {
            let mut reads = ReadsBuilder::new();
            reads.record(key, ContentHash::of_bytes(value));
            producer
                .fact(
                    Address(ids[0].path().fingerprint()),
                    summary(ids[0], vec![]),
                    &reads.finish().unwrap(),
                    selected,
                )
                .unwrap()
        };
        let a = make(b"before");
        let b = make(b"after");
        assert_ne!(a.product.action, b.product.action);
        assert_eq!(a.product.record, b.product.record);
        assert_eq!(
            store.lookup_product(b.product.address).unwrap().action,
            b.product.action
        );
    }
}

#[test]
fn independently_selected_roots_sharing_a_lexical_owner_all_contribute() {
    let (engine, source, ids, sites) = context();
    let store = crate::store::MemStore::default();
    let records = NativeRecords::new(
        engine.types().clone(),
        Arc::new(crate::store::MemStore::default()),
    );
    let producer = SourceGrowthProducer {
        records: &records,
        source: &source,
        store: &store,
        namespace: Address(ContentHash::of_bytes(b"shared lexical owner")),
        scratch: 8 << 20,
    };
    let reads = ReadsBuilder::new().finish().unwrap();
    let selected = FACT.reference(b"accepted checked output").unwrap();
    let a = producer
        .fact(
            Address(ids[0].path().fingerprint()),
            summary(ids[0], vec![]),
            &reads,
            selected,
        )
        .unwrap();
    let b = producer
        .fact(
            Address(ids[1].path().fingerprint()),
            summary(
                ids[0],
                vec![(parameter(ids[0]), parameter(ids[0]), true, sites[0])],
            ),
            &reads,
            selected,
        )
        .unwrap();
    assert_ne!(a.product.address, b.product.address);
    let facets = producer.settle(&[a, b]).unwrap();
    assert_eq!(facets.components.len(), 1);
    assert_eq!(&*facets.components[0].growing, &[sites[0]]);
    let bounded = SourceGrowthProducer {
        scratch: 1,
        ..producer
    };
    assert!(bounded.settle(&[]).is_none());
}
