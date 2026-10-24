use hirsel_driver::*;
use hirsel_engine::*;
use native::queries::declarations::NativeDeclarations;
use std::sync::Arc;

fn session(
    store: Arc<store::DirStore>,
    literal: u64,
    lower: bool,
) -> (QueryEngine, NativeDeclarations) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
        .with_result_store(store.clone());
    let file = engine
        .sources
        .add(
            "worker-lower.hir",
            format!("pub fn main()->u64 {{{literal}u64}}\n"),
        )
        .unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    if !lower {
        engine.adapters.lowering = None;
    }
    let mut queries = NativeDiscoveryCache::new(engine.types().clone(), store);
    let declarations = NativeDeclarations::collect(&mut engine, &mut queries, &[]).unwrap();
    (engine, declarations)
}
fn literal(body: &hirsel_tir::Body) -> u64 {
    body.rvalues
        .iter()
        .find_map(|value| {
            if let hirsel_tir::Rvalue::Constant(hirsel_tir::Constant::Integer(n)) = value {
                Some(*n as u64)
            } else {
                None
            }
        })
        .unwrap()
}
#[test]
fn native_lower_reader_selects_actual_current_v_and_releases_original_sources_before_worker_decode()
{
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(directory.path().join("products")));
        let (mut engine, mut declarations) = session(store.clone(), 7, true);
        let main = entry_path(&default_context());
        let original = declarations.body(&mut engine, &main).unwrap();
        let weak = Arc::downgrade(&original);
        let output = declarations.sources[&main].lower.clone().unwrap();
        let revision = declarations.sources[&main].source.implementation_revision;
        let base = hirsel_layout::instance::InstanceKey::new(
            &main,
            original.entry.0,
            &[],
            engine.types(),
            None,
            None,
        )
        .unwrap();
        let reader = engine
            .native_lower_reader(&declarations.roots.definitions)
            .unwrap()
            .unwrap();
        fn send_sync<T: Send + Sync>(_: &T) {}
        send_sync(&reader);
        assert!(reader.contains(&main));
        assert!(!reader.contains(&hirsel_syntax::DefPath::core("unselected")));
        drop(original);
        drop(declarations);
        drop(engine);
        assert!(
            weak.upgrade().is_none(),
            "immutable worker inputs retain no original Definition"
        );
        let trace = hirsel_lir::construction::ValueTrace::default();
        let _observe = trace.enter();
        let demanded = reader.read(&main).unwrap();
        assert_eq!(demanded.output().owner(), output.owner());
        assert_eq!(demanded.output().reference(), output.reference());
        assert_eq!(
            demanded.output().observation_identity(),
            output.observation_identity()
        );
        assert_eq!(demanded.implementation_revision(), revision);
        assert_eq!(
            literal(&demanded.definition().bodies[demanded.definition().entry.0 as usize]),
            7
        );
        let local = hirsel_rc::sealed_source::SourceInput::capture(
            demanded.output().observation_identity(),
            demanded.implementation_revision(),
            demanded.definition().clone(),
        );
        assert_eq!(literal(local.demand_body(&base).unwrap()), 7);
        let decoded = Arc::downgrade(demanded.definition());
        drop(demanded);
        assert!(decoded.upgrade().is_some());
        drop(local);
        assert!(
            decoded.upgrade().is_none(),
            "demanded source is released with its local native input"
        );
        assert!(
            reader
                .read(&hirsel_syntax::DefPath::core("unselected"))
                .is_none()
        );
        assert_eq!(trace.totals(), Default::default());
        // The same cache in a fresh source session must admit its current V
        // output with the Lower producer unavailable and no old Definition.
        let (mut fresh, declarations) = session(store.clone(), 7, false);
        let current = fresh
            .native_lower_reader(&declarations.roots.definitions)
            .unwrap()
            .unwrap();
        drop(declarations);
        drop(fresh);
        let selected = current.read(&main).unwrap();
        assert_eq!(selected.output().owner(), output.owner());
        assert_eq!(selected.output().reference(), output.reference());
        assert_eq!(
            selected.output().observation_identity(),
            output.observation_identity()
        );
        // An equal declaration signature cannot select an older body. The old
        // reader still denotes its own immutable source snapshot independently.
        let (mut changed, declarations) = session(store.clone(), 8, true);
        let replacement = changed
            .native_lower_reader(&declarations.roots.definitions)
            .unwrap()
            .unwrap();
        drop(declarations);
        drop(changed);
        let newer = replacement.read(&main).unwrap();
        assert_ne!(
            newer.output().observation_identity(),
            output.observation_identity()
        );
        assert_eq!(
            literal(&newer.definition().bodies[newer.definition().entry.0 as usize]),
            8
        );
        assert_eq!(
            literal(&reader.read(&main).unwrap().definition().bodies[0]),
            7
        );
        assert_eq!(trace.totals(), Default::default());
    });
}

#[test]
fn native_lower_reader_never_acquires_authority_from_verify_off_or_absent_local_verifier() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(directory.path().join("products")));
        let (mut engine, declarations) = session(store, 7, true);
        assert!(
            declarations
                .sources
                .values()
                .any(|source| source.lower.is_some())
        );
        engine.verify = Verify::Off;
        assert!(
            engine
                .native_lower_reader(&declarations.roots.definitions)
                .unwrap()
                .is_none(),
            "previous actual V outputs cannot make a VerifyOff worker reader"
        );
        let mut absent = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(Arc::new(store::NoneStore));
        assert!(
            absent
                .native_lower_reader(&declarations.roots.definitions)
                .unwrap()
                .is_none()
        );
    });
}

#[test]
fn native_lower_reader_uses_current_generated_lower_at_equal_signatures() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(directory.path().join("products")));
        let session = |expression: &str, lower: bool| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let source = format!(
                "type P {{n:u64}}\nfn gen[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {{}}],String] {{Ok(codegen.body(\"fn(v:standalone@0/P)->u64 uses {{}} {{{expression}}}\"))}}\nfn chosen[T](v:T)->u64 uses {{}} = comptime gen(shape(T))\npub fn main()->u64 uses {{}} {{chosen(P(n:3u64))}}"
            );
            let file = engine.sources.add("reader-generated.hir", source).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            if !lower {
                engine.adapters.lowering = None;
            }
            let mut cache = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
            let declarations = NativeDeclarations::collect(&mut engine, &mut cache, &[]).unwrap();
            let selected = declarations
                .sources
                .keys()
                .find(|p| p.0.name.starts_with("chosen$instance$"))
                .unwrap()
                .clone();
            let signature = declarations.sources[&selected].signature.fingerprint;
            let reader = engine
                .native_lower_reader(&declarations.roots.definitions)
                .unwrap()
                .unwrap();
            (reader, selected, signature)
        };
        let (first, selected, signature) = session("v.n", true);
        let body = first.read(&selected).unwrap();
        let (same, owner, header) = session("v.n", false);
        assert_eq!(owner, selected);
        assert_eq!(signature, header);
        assert_eq!(same.read(&selected).unwrap().output(), body.output());
        let (changed, owner, header) = session("v.n+1u64", true);
        assert_eq!(owner, selected);
        assert_eq!(signature, header);
        assert_ne!(
            changed
                .read(&selected)
                .unwrap()
                .output()
                .observation_identity(),
            body.output().observation_identity()
        );
        assert_eq!(
            first.read(&selected).unwrap().output(),
            body.output(),
            "the earlier reader keeps its own source snapshot"
        );
    });
}
