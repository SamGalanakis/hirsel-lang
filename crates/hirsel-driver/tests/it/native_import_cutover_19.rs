//! Import-cache vectors exercised through the demand producer before the old
//! whole-program replay provider is removed.
use hirsel_driver::*;
use hirsel_engine::{store::ResultStore, *};
use native::queries::{
    declarations::NativeDeclarations, demand::DemandInstances, source_inputs::*,
};
use std::{collections::BTreeMap, sync::Arc};

type Bodies = BTreeMap<Vec<u8>, (Vec<u8>, Vec<u8>)>;

fn import(source: &str, store: Arc<dyn ResultStore>) -> (Bodies, u64, u64) {
    import_with(source, store, |_| {})
}

fn import_with(
    source: &str,
    store: Arc<dyn ResultStore>,
    prepare: impl FnOnce(&mut QueryEngine),
) -> (Bodies, u64, u64) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
        .with_result_store(store.clone());
    let file = engine.sources.add("imports.hir", source.into()).unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    prepare(&mut engine);
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    import_engine(engine, &[], store)
}

fn import_engine(
    engine: QueryEngine,
    roots: &[DefPath],
    store: Arc<dyn ResultStore>,
) -> (Bodies, u64, u64) {
    import_engine_with_validation(engine, roots, store, native::Validation::Ready)
}

fn import_engine_with_validation(
    mut engine: QueryEngine,
    roots: &[DefPath],
    store: Arc<dyn ResultStore>,
    validation: native::Validation,
) -> (Bodies, u64, u64) {
    let budget = runtime::ProcessBudget::new(64 << 20);
    let store: Arc<dyn ResultStore> = Arc::new(store::RequestStore::new(
        store,
        budget.reserve(64 << 20).unwrap(),
    ));
    let facts = crate::native_front_fixture::products(&mut engine, roots).unwrap();
    let declarations = NativeDeclarations::from_front(facts.clone()).unwrap();
    let reader = facts
        .original_context
        .reader(facts.originals.iter().map(|product| (**product).clone()))
        .unwrap();
    drop(facts);
    let inputs = NativeInstanceInputs::from_products(declarations, reader).unwrap();
    drop(engine);
    let mut source = inputs.provider();
    let target = native::TargetSpec::x86_64_unknown_linux_gnu();
    let policy = hirsel_syntax::ContentHash::of_bytes(b"native import cutover control");
    let mut producer =
        DemandInstances::new_from_source(&source, store.clone(), target.clone(), policy);
    let mut pending = producer.roots_from(&source).unwrap();
    let mut bodies = BTreeMap::new();
    let mut options = native::Options::new(target);
    options.validation = validation;
    while let Some(key) = pending.pop() {
        if bodies.contains_key(key.instance.encoding()) {
            continue;
        }
        let output = producer.instance_from(&mut source, &key, &options).unwrap();
        let value = store.get_content(output.product.value).unwrap().to_vec();
        let layouts = store
            .get_content(output.product.layout_closure)
            .unwrap()
            .to_vec();
        pending.extend(output.facts.body.dependencies.iter().cloned());
        bodies.insert(key.instance.encoding().to_vec(), (value, layouts));
    }
    assert!(!bodies.is_empty());
    (bodies, producer.imports, producer.counts().hits)
}

fn fresh_import(source: &str) -> (Bodies, u64, u64) {
    let dir = tempfile::tempdir().unwrap();
    import(
        source,
        Arc::new(store::DirStore::new(dir.path().join("products"))),
    )
}

#[test]
fn demand_imported_closure_symbols_match_fresh_after_layout_numbering_changes() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(dir.path().join("products")));
        let source = "type Captured { n:u64 }\nfn compute(x:Captured)->u64 { let f=fn()->u64 { x.n }; f() }\nfn main()->u64 { compute(Captured(n:3u64)) }\n";
        let donor = format!("fn aaa()->(bool,u32) {{ (true,0u32) }}\n{source}");
        assert!(import(&donor, store.clone()).1 > 0);
        let cached = import(source, store);
        let fresh = fresh_import(source);
        assert!(
            cached.2 > 0,
            "the distinct consumer must exercise stored imports"
        );
        assert_eq!(
            cached.0, fresh.0,
            "canonical symbols, bodies and reached layouts must match fresh imports"
        );
    });
}

#[test]
fn demand_imports_replay_across_sessions_unrelated_layouts_and_body_edits() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(dir.path().join("products")));
        let source = "fn helper(x: u64) -> u64 { x + 1u64 }\nfn main() -> u64 { helper(3u64) }\n";
        let cold = import(source, store.clone());
        assert_eq!(cold.2, 0);
        assert_eq!(cold.1, cold.0.len() as u64);
        let warm = import(source, store.clone());
        assert_eq!(warm.0, cold.0);
        assert_eq!(warm.1, 0);
        assert_eq!(warm.2, cold.0.len() as u64);
        let unrelated = format!(
            "type Other {{ text: String }}\nfn unrelated() -> Other {{ Other(text: \"x\") }}\n{source}"
        );
        let shifted = import(&unrelated, store.clone());
        for (key, body) in &cold.0 {
            assert_eq!(
                shifted.0.get(key),
                Some(body),
                "unrelated layouts changed an existing import"
            );
        }
        assert!(shifted.2 >= cold.0.len() as u64);
        let edited = unrelated.replace("x + 1u64", "x + 2u64");
        let edit = import(&edited, store.clone());
        assert_eq!(edit.1, 1, "only the helper's body changed");
        assert_eq!(edit.0.len(), shifted.0.len());
        assert_eq!(edit.0, fresh_import(&edited).0);
        let lists = "fn main() -> u64 { let mut xs: List[u64] = []; list.push(mut xs, 1u64); list.len(xs) }";
        let list_cold = import(lists, store.clone());
        assert!(list_cold.1 > 0);
        let list_warm = import(lists, store);
        assert_eq!(list_warm.0, list_cold.0);
        assert_eq!(list_warm.1, 0);
        assert_eq!(list_warm.2, list_warm.0.len() as u64);
    });
}

#[test]
fn demand_import_products_relocate_effect_types_without_session_ids() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(dir.path().join("products")));
        let source = "type P { n:u64 }\nfn keep(x:List[P])->List[P] in x { x }\nfn main()->u64 { let xs=keep([P(n:3u64)]); xs[0u64].n }";
        let mut donor = None;
        let cold = import_with(source, store.clone(), |engine| {
            let signature = engine
                .item_signature(&{
                    let mut path = entry_path(&default_context());
                    path.0.name = "keep".into();
                    path
                })
                .unwrap()
                .value;
            let hirsel_types::SignatureKind::Function(function) = &signature.kind else {
                panic!("function")
            };
            donor = Some(function.parameters[0].ty);
        });
        let mut consumer = None;
        let warm = import_with(source, store, |engine| {
            for size in 0..128 {
                engine.types().intern(hirsel_types::Type::with_kind(
                    hirsel_types::TypeKind::Tuple(
                        vec![engine.types().intern(hirsel_types::Type::boolean()); size].into(),
                    ),
                ));
            }
            let mut path = entry_path(&default_context());
            path.0.name = "keep".into();
            let signature = engine.item_signature(&path).unwrap().value;
            let hirsel_types::SignatureKind::Function(function) = &signature.kind else {
                panic!("function")
            };
            consumer = Some(function.parameters[0].ty);
        });
        assert_ne!(donor, consumer, "the consumer must actually shift TypeIds");
        assert_eq!(cold.0, warm.0);
        assert_eq!(warm.1, 0);
        assert_eq!(warm.2, warm.0.len() as u64);
    });
}

#[test]
fn demand_http1_seed_imports_replay_complete_layouts() {
    with_compiler_stack(|| {
        let manifest =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/std@1/hirsel.json");
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(dir.path().join("products")));
        let load = || {
            let mut package = COMPILER.load_package_declarations(&manifest).unwrap();
            package.engine = package.engine.with_result_store(store.clone());
            package
        };
        let mut package = load();
        let mut roots = Vec::new();
        for (file, context) in package.files.clone() {
            if context.modules != ["internal", "http1_parse"] {
                continue;
            }
            let syntax = package.engine.parse_file(file, &context).unwrap().value;
            for item in &syntax.items {
                if let hirsel_syntax::ItemKind::Function {
                    signature,
                    body: Some(_),
                    ..
                } = &item.item.kind
                    && signature.generics.is_empty()
                {
                    roots.push(item.item.definition.clone());
                }
            }
        }
        roots.sort();
        roots.dedup();
        assert_eq!(roots.len(), 13);
        let donor = DefPath(hirsel_syntax::ItemPath {
            package: package.package.clone(),
            modules: vec!["http".into()],
            name: "http_server_reject".into(),
        });
        import_engine(package.engine, &[donor], store.clone());
        let cached = import_engine(load().engine, &roots, store.clone());
        assert!(cached.2 > 0);
        let mut fresh = COMPILER.load_package_declarations(&manifest).unwrap();
        let fresh_dir = tempfile::tempdir().unwrap();
        let fresh_store = Arc::new(store::DirStore::new(fresh_dir.path().join("products")));
        fresh.engine = fresh.engine.with_result_store(fresh_store.clone());
        assert_eq!(cached.0, import_engine(fresh.engine, &roots, fresh_store).0);
    });
}

struct FaultStore {
    rehash: bool,
    inner: store::DirStore,
    seen: std::sync::Mutex<Vec<store::Reference>>,
    damaged: std::sync::Mutex<BTreeMap<hirsel_syntax::ContentHash, Vec<u8>>>,
}
impl ResultStore for FaultStore {
    fn local_product_store(&self) -> Option<hirsel_artifact_store::Store> {
        self.inner.local_product_store()
    }
    fn verifier(&self) -> Option<Arc<hirsel_validate::VerifierCache>> {
        self.inner.verifier()
    }
    fn get(&self, key: &store::EntryKey) -> Option<store::Bytes> {
        self.inner.get(key)
    }
    fn put(&self, key: &store::EntryKey, value: &[u8]) {
        self.inner.put(key, value)
    }
    fn lookup_product(&self, address: store::Address) -> Option<store::Association> {
        let mut association = self.inner.lookup_product(address)?;
        if self.rehash
            && let Some(bytes) = self
                .damaged
                .lock()
                .unwrap()
                .get(&association.reference.output.0)
        {
            // Supply ordinary content with a valid hash: the consumer still
            // must parse its schema and independently admit native C.
            association.reference = self
                .inner
                .put_content(association.reference.domain, bytes)?;
            assert!(self.inner.get_content(association.reference).is_some());
        }
        Some(association)
    }
    fn associate_product(
        &self,
        address: store::Address,
        action: store::Action,
        reference: store::Reference,
    ) {
        self.inner.associate_product(address, action, reference)
    }
    fn get_content(&self, reference: store::Reference) -> Option<store::Bytes> {
        if let Some(bytes) = self.damaged.lock().unwrap().get(&reference.output.0) {
            return (reference.domain.reference(bytes)? == reference).then(|| bytes.clone().into());
        }
        self.inner.get_content(reference)
    }
    fn put_content(&self, domain: store::Domain, bytes: &[u8]) -> Option<store::Reference> {
        let reference = self.inner.put_content(domain, bytes)?;
        self.damaged.lock().unwrap().remove(&reference.output.0);
        let mut seen = self.seen.lock().unwrap();
        if !seen.contains(&reference) {
            seen.push(reference);
        }
        Some(reference)
    }
    fn name(&self) -> &'static str {
        "import-cutover-faults"
    }
}

#[test]
fn demand_import_corruption_misses_then_overwrites_and_reopens() {
    with_compiler_stack(|| {
        // Preserve the two old framing/effect JSON controls' source vectors.
        for source in [
            "type P { n:u64 }\nfn helper(x:P)->u64 { x.n + 1u64 }\nfn main()->u64 { helper(P(n:3u64)) }",
            "type P { n:u64 }\nfn keep(x:List[P])->List[P] in x { x }\nfn main()->u64 { let xs=keep([P(n:3u64)]); xs[0u64].n }",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let store = Arc::new(FaultStore {
                rehash: false,
                inner: store::DirStore::new(dir.path().join("products")),
                seen: Default::default(),
                damaged: Default::default(),
            });
            let expected = import(source, store.clone()).0;
            let records = store
                .seen
                .lock()
                .unwrap()
                .iter()
                .copied()
                .filter(|r| r.domain == native::queries::instance::INSTANCE_RECORD_DOMAIN)
                .collect::<Vec<_>>();
            assert!(!records.is_empty());
            for kind in 0..9 {
                for &reference in &records {
                    let mut bytes = store.inner.get_content(reference).unwrap().to_vec();
                    match kind {
                        0 => bytes = br#"["legacy",[],[],[]]"#.to_vec(),
                        1 => bytes[0] ^= 0xff,
                        2 => {
                            bytes.pop();
                        }
                        3 => bytes.push(0),
                        4 => bytes = b"{".to_vec(),
                        5 => bytes = b"null".to_vec(),
                        6 => bytes = b"[] []".to_vec(),
                        7 => {
                            let mut fields: Vec<store::Reference> =
                                serde_json::from_slice(&bytes).unwrap();
                            fields.swap(0, 4);
                            bytes = serde_json::to_vec(&fields).unwrap();
                        }
                        _ => {
                            let mut fields: Vec<store::Reference> =
                                serde_json::from_slice(&bytes).unwrap();
                            fields[0].domain.schema += 1;
                            bytes = serde_json::to_vec(&fields).unwrap();
                        }
                    }
                    store
                        .damaged
                        .lock()
                        .unwrap()
                        .insert(reference.output.0, bytes);
                    assert!(store.get_content(reference).is_none(), "damage {kind}");
                }
                let rebuilt = import(source, store.clone());
                assert_eq!(rebuilt.0, expected);
                assert_eq!(rebuilt.1, expected.len() as u64, "damage {kind} must miss");
                assert_eq!(rebuilt.2, 0);
                assert!(store.damaged.lock().unwrap().is_empty());
                let reopened = import(source, store.clone());
                assert_eq!(reopened.0, expected);
                assert_eq!(reopened.1, 0);
                assert_eq!(reopened.2, expected.len() as u64);
            }
        }
    });
}

#[test]
fn demand_import_malformed_descriptor_is_not_native_authority() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FaultStore {
            rehash: true,
            inner: store::DirStore::new(dir.path().join("products")),
            seen: Default::default(),
            damaged: Default::default(),
        });
        let source = "type P { n:u64 }\nfn helper(x:P)->u64 { x.n + 1u64 }\nfn main()->u64 { helper(P(n:3u64)) }";
        let expected = import(source, store.clone()).0;
        let records = store
            .seen
            .lock()
            .unwrap()
            .iter()
            .copied()
            .filter(|r| r.domain == native::queries::instance::INSTANCE_RECORD_DOMAIN)
            .collect::<Vec<_>>();
        assert!(!records.is_empty());
        for malformed in [
            &b"{"[..],
            &b"null"[..],
            &b"[] []"[..],
            &br#"["legacy",[],[],[]]"#[..],
        ] {
            for reference in &records {
                store
                    .damaged
                    .lock()
                    .unwrap()
                    .insert(reference.output.0, malformed.to_vec());
            }
            let rebuilt = import(source, store.clone());
            assert_eq!(rebuilt.0, expected);
            assert_eq!(rebuilt.1, expected.len() as u64);
            assert_eq!(rebuilt.2, 0);
            assert!(store.damaged.lock().unwrap().is_empty());
            let reopened = import(source, store.clone());
            assert_eq!(reopened.0, expected);
            assert_eq!(reopened.1, 0);
            assert_eq!(reopened.2, expected.len() as u64);
        }
        for kind in 0..2 {
            for &reference in &records {
                let bytes = store.inner.get_content(reference).unwrap();
                let mut fields: Vec<store::Reference> = serde_json::from_slice(&bytes).unwrap();
                if kind == 0 {
                    fields.swap(0, 4);
                } else {
                    fields[0].domain.schema += 1;
                }
                store
                    .damaged
                    .lock()
                    .unwrap()
                    .insert(reference.output.0, serde_json::to_vec(&fields).unwrap());
            }
            let rebuilt = import(source, store.clone());
            assert_eq!(rebuilt.0, expected);
            assert_eq!(rebuilt.1, expected.len() as u64);
            assert_eq!(rebuilt.2, 0);
            assert!(store.damaged.lock().unwrap().is_empty());
        }
    });
}

#[test]
fn demand_import_every_stage_reimports_warm_products() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(dir.path().join("products")));
        let source = "fn helper(x: u64) -> u64 { x + 1u64 }\nfn main() -> u64 { helper(3u64) }\n";
        let expected = import(source, store.clone()).0;
        let warm = import(source, store.clone());
        assert_eq!(warm.1, 0);
        assert_eq!(warm.2, expected.len() as u64);
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine.sources.add("imports.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let forced =
            import_engine_with_validation(engine, &[], store, native::Validation::EveryStage);
        assert_eq!(forced.0, expected);
        assert_eq!(
            forced.1,
            expected.len() as u64,
            "EveryStage must execute every importer despite warm products"
        );
        assert_eq!(forced.2, 0);
    });
}

/// Exact former wire vectors remain negative inputs at the replacement boundary.
pub(crate) fn assert_retired_import_payloads(payloads: &[Vec<u8>]) {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FaultStore {
            rehash: true,
            inner: store::DirStore::new(dir.path().join("products")),
            seen: Default::default(),
            damaged: Default::default(),
        });
        let source = "fn helper(x:u64)->u64{x+1u64}\nfn main()->u64{helper(3u64)}";
        let expected = import(source, store.clone()).0;
        let reference = *store
            .seen
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.domain == native::queries::instance::INSTANCE_RECORD_DOMAIN)
            .unwrap();
        for payload in payloads {
            store
                .damaged
                .lock()
                .unwrap()
                .insert(reference.output.0, payload.clone());
            let rebuilt = import(source, store.clone());
            assert_eq!(rebuilt.0, expected);
            assert_eq!(
                rebuilt.1, 1,
                "a retired wire payload must miss rather than be accepted"
            );
            assert!(store.damaged.lock().unwrap().is_empty());
        }
        assert_eq!(import(source, store).1, 0);
    });
}
