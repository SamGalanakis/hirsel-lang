use super::*;

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
