use hirsel_driver::*;
use hirsel_engine::*;
use native::queries::declarations::NativeDeclarations;
use std::sync::Arc;

type EntryRecord = (
    hirsel_types::FunctionType,
    hirsel_types::SiteRef,
    hirsel_span::Span,
);

fn source(store: Arc<store::MemStore>, path: &str) -> (QueryEngine, NativeDeclarations) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
        .with_result_store(store.clone());
    let file = engine
        .sources
        .add(
            path,
            "fn id[T](take x:T)->T{take x}\npub fn main()->u64 {id(take 7u64)}".into(),
        )
        .unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    let mut queries = NativeDiscoveryCache::new(engine.types().clone(), store);
    let declarations = NativeDeclarations::collect(&mut engine, &mut queries, &[]).unwrap();
    (engine, declarations)
}
#[test]
fn immutable_native_codec_context_matches_live_source_and_outlives_original_definition() {
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let (mut engine, mut declarations) = source(store.clone(), "first/records.hir");
        let generic = declarations
            .sources
            .keys()
            .find(|path| path.0.name == "id")
            .unwrap()
            .clone();
        let definition = declarations.body(&mut engine, &generic).unwrap();
        let weak = Arc::downgrade(&definition);
        let hirsel_types::SignatureKind::Function(function) =
            &declarations.sources[&generic].signature.kind
        else {
            panic!("function signature")
        };
        let record: EntryRecord = (
            function.clone(),
            definition.bodies[definition.entry.0 as usize].parameter_scope,
            declarations.sources[&generic].source.syntax.item.name_span,
        );
        let records = NativeRecords::new(engine.types().clone(), store.clone());
        let context = NativeRecordContext::capture(&engine);
        fn send_sync<T: Send + Sync>(_: &T) {}
        send_sync(&context);
        let live = records.encode(&engine, &record).unwrap();
        assert_eq!(records.encode(&context, &record).unwrap(), live);
        assert_eq!(
            records.decode::<EntryRecord>(&engine, &live).unwrap(),
            record
        );
        assert_eq!(
            records.decode::<EntryRecord>(&context, &live).unwrap(),
            record
        );
        drop(definition);
        drop(declarations);
        drop(engine);
        assert!(
            weak.upgrade().is_none(),
            "worker codec retains no original Definition"
        );
        let restored: EntryRecord = records.decode(&context, &live).unwrap();
        assert_eq!(restored, record);
        assert_eq!(records.encode(&context, &restored).unwrap(), live);
        // A new source snapshot rebinds original owners at relocated numeric
        // anchors. A different/absent source cannot supply an old owner.
        let (mut relocated, mut declarations) = source(store.clone(), "other/records.hir");
        let original = declarations.body(&mut relocated, &generic).unwrap();
        let relocated_context = NativeRecordContext::capture(&relocated);
        let relocated_records = NativeRecords::new(relocated.types().clone(), store.clone());
        let rebound: EntryRecord = relocated_records.decode(&relocated_context, &live).unwrap();
        assert_eq!(
            rebound.1,
            original.bodies[original.entry.0 as usize].parameter_scope
        );
        assert_eq!(
            rebound.2,
            declarations.sources[&generic].source.syntax.item.name_span
        );
        assert_eq!(
            relocated_records
                .encode(&relocated_context, &rebound)
                .unwrap(),
            live
        );
        assert!(
            records.encode(&relocated_context, &record).is_none(),
            "foreign interner is rejected"
        );
        let absent = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let absent_context = NativeRecordContext::capture(&absent);
        let absent_records = NativeRecords::new(absent.types().clone(), store);
        assert!(
            absent_records
                .decode::<EntryRecord>(&absent_context, &live)
                .is_none()
        );
    });
}

#[test]
fn immutable_native_codec_context_binds_actual_generated_text_and_refuses_missing_generation() {
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let session = |expression: &str, generate: bool| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let text = format!(
                "type P {{n:u64}}\nfn gen[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {{}}],String] {{Ok(codegen.body(\"fn(v:standalone@0/P)->u64 uses {{}} {{{expression}}}\"))}}\nfn chosen[T](v:T)->u64 uses {{}} = comptime gen(shape(T))\npub fn main()->u64 uses {{}} {{chosen(P(n:3u64))}}"
            );
            let file = engine.sources.add("generated-records.hir", text).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            let mut queries = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
            let declarations = if generate {
                Some(NativeDeclarations::collect(&mut engine, &mut queries, &[]).unwrap())
            } else {
                None
            };
            (engine, declarations)
        };
        let (first, declarations) = session("v.n", true);
        let declarations = declarations.unwrap();
        let selected = declarations
            .sources
            .keys()
            .find(|path| path.0.name.starts_with("chosen$instance$"))
            .unwrap()
            .clone();
        let signature = declarations.sources[&selected].signature.fingerprint;
        let generated = declarations.sources[&selected]
            .source
            .generated
            .as_ref()
            .unwrap()
            .file;
        let context = NativeRecordContext::capture(&first);
        let anchor = first
            .sources
            .anchors()
            .find(|(_, file, _)| *file == generated)
            .unwrap()
            .0;
        assert!(
            context
                .record_owner(anchor)
                .unwrap()
                .0
                .0
                .name
                .starts_with("$generated-source:")
        );
        let span = hirsel_span::Span::new(anchor, 0, 0).unwrap();
        let records = NativeRecords::new(first.types().clone(), store.clone());
        let bytes = records.encode(&first, &span).unwrap();
        assert_eq!(records.encode(&context, &span).unwrap(), bytes);
        drop(declarations);
        drop(first);
        assert_eq!(
            records
                .decode::<hirsel_span::Span>(&context, &bytes)
                .unwrap(),
            span
        );
        let (same, declarations) = session("v.n", true);
        assert_eq!(
            declarations.as_ref().unwrap().sources[&selected]
                .signature
                .fingerprint,
            signature
        );
        let same_context = NativeRecordContext::capture(&same);
        let same_records = NativeRecords::new(same.types().clone(), store.clone());
        let rebound: hirsel_span::Span = same_records.decode(&same_context, &bytes).unwrap();
        assert_eq!(same_records.encode(&same_context, &rebound).unwrap(), bytes);
        let (missing, _) = session("v.n", false);
        let missing_context = NativeRecordContext::capture(&missing);
        let missing_records = NativeRecords::new(missing.types().clone(), store.clone());
        assert!(
            missing_records
                .decode::<hirsel_span::Span>(&missing_context, &bytes)
                .is_none()
        );
        let (changed, declarations) = session("v.n+1u64", true);
        assert_eq!(
            declarations.as_ref().unwrap().sources[&selected]
                .signature
                .fingerprint,
            signature
        );
        let changed_context = NativeRecordContext::capture(&changed);
        let changed_records = NativeRecords::new(changed.types().clone(), store);
        assert!(
            changed_records
                .decode::<hirsel_span::Span>(&changed_context, &bytes)
                .is_none()
        );
    });
}
