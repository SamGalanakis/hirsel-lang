use hirsel_driver::*;
use hirsel_engine::{runtime::*, *};
use hirsel_tir::TirProvider;
use std::sync::Arc;
fn signature_session(
    store: Arc<dyn store::ResultStore>,
    source: &str,
    normalize: bool,
) -> QueryEngine {
    let mut adapters = compiler_adapters();
    if !normalize {
        adapters.signature = None;
    }
    let mut engine = QueryEngine::new(default_environment(), adapters).with_result_store(store);
    if !normalize {
        engine
            .sources
            .add("earlier.hir", "fn unrelated(){}\n".into())
            .unwrap();
    }
    let file = engine.sources.add("signatures.hir", source.into()).unwrap();
    assert!(
        engine
            .parse_file(file, &default_context())
            .unwrap()
            .diagnostics
            .is_empty()
    );
    engine
}
fn definition(name: &str) -> DefPath {
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    path
}
#[test]
fn staged_lower_products_restore_own_declarations_before_admission_without_modes() {
    with_compiler_stack(|| {
        use crate::staged_product_fixture::StagedProducts;
        let source = "pub opaque type Hidden:plain uses {}{field:u32}\nfn value[n:size]()->u64{comptime (n*n)}\n";
        let path = definition("value");
        let store = Arc::new(store::MemStore::default());
        let mut seed = signature_session(store.clone(), source, true);
        let mut seed_products = StagedProducts::capture(&mut seed, 1).unwrap();
        let request = seed_products
            .runtime
            .request(&[seed_products.lower.query(&path).unwrap()]);
        seed_products.runtime.wait(request).unwrap();
        let expected = seed_products.lower.settled_definition(&path).unwrap();
        let declaration = expected
            .bodies
            .iter()
            .flat_map(|body| &body.rvalues)
            .find_map(|value| match value {
                hirsel_tir::Rvalue::ReadConstant {
                    definition,
                    arguments,
                } if !arguments.is_empty() => Some(definition.clone()),
                _ => None,
            })
            .unwrap();
        let signature = seed.item_signature(&declaration).unwrap().value.fingerprint;
        seed_products.runtime.shutdown();
        drop(seed_products);
        drop(seed);
        let mut fresh = signature_session(store.clone(), source, false);
        fresh.adapters.modes = None;
        fresh.adapters.lowering = None;
        let mut products = StagedProducts::capture(&mut fresh, 1).unwrap();
        let query = products.lower.query(&path).unwrap();
        let request = products.runtime.request(&[query]);
        products.runtime.wait(request).unwrap();
        let actual = products.lower.settled_definition(&path).unwrap();
        assert_eq!(actual.path, expected.path);
        assert_eq!(
            fresh
                .item_signature(&declaration)
                .unwrap()
                .value
                .fingerprint,
            signature
        );
        assert_eq!(products.lower.work(&path).lowerings, 0);
        assert!(
            !products
                .front
                .work_snapshot()
                .iter()
                .any(|(p, kind, count)| p == &path && *kind == QueryKind::Modes && *count != 0)
        );
        products.runtime.shutdown();
        drop(products);
        drop(fresh);
        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
            store::ResultStore::put(&*store, &key, &bytes);
        }
        for (provider, text, normalize, message) in [
            (
                copied as Arc<dyn store::ResultStore>,
                source.to_owned(),
                false,
                "declaration bytes cannot transfer V completion",
            ),
            (
                store.clone(),
                source.replace("field:u32", "field:u64"),
                true,
                "private V universe changes invalidate completion",
            ),
        ] {
            let mut invalid = signature_session(provider, &text, normalize);
            invalid.adapters.modes = None;
            invalid.adapters.lowering = None;
            let accepted = match StagedProducts::capture(&mut invalid, 1) {
                Err(_) => false,
                Ok(mut products) => {
                    let request = products
                        .runtime
                        .request(&[products.lower.query(&path).unwrap()]);
                    let accepted = products.runtime.wait(request).is_ok();
                    products.runtime.shutdown();
                    accepted
                }
            };
            assert!(!accepted, "{message}");
        }
        let mut oracle =
            signature_session(store, source, true).with_verify_store(store::VerifyStore::Recompute);
        oracle.lower_to_tir(&path).unwrap();
    });
}

#[test]
fn lower_hits_replay_exact_staged_values_and_preserve_completion_authority() {
    with_compiler_stack(|| {
        use crate::staged_product_fixture::StagedProducts;
        let source = "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime (n*n)}\nfn value()->u64{comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)}}\n";
        let root = definition("value");
        let store = Arc::new(store::MemStore::default());
        let mut seed = signature_session(store.clone(), source, true);
        let mut seed_products = StagedProducts::capture(&mut seed, 1).unwrap();
        let request = seed_products.runtime.request(&[
            seed_products.lower.query(&definition("scaled")).unwrap(),
            seed_products.lower.query(&root).unwrap(),
        ]);
        seed_products.runtime.wait(request).unwrap();
        let generic = seed_products
            .lower
            .settled_definition(&definition("scaled"))
            .unwrap();
        let hidden = generic.bodies[generic.entry.0 as usize]
            .rvalues
            .iter()
            .find_map(|r| match r {
                hirsel_tir::Rvalue::ReadConstant {
                    definition,
                    arguments,
                } if !arguments.is_empty() => Some(definition.clone()),
                _ => None,
            })
            .unwrap();
        let expected = seed_products.lower.settled_definition(&root).unwrap();
        assert!(seed_products.budget.reserved() > 0);
        seed_products.runtime.shutdown();
        drop(seed_products);
        drop(seed);
        let mut fresh = signature_session(store.clone(), source, false);
        fresh.adapters.modes = None;
        fresh.adapters.lowering = None;
        let mut products = StagedProducts::capture(&mut fresh, 1).unwrap();
        let query = products.lower.query(&root).unwrap();
        for _ in 0..2 {
            let request = products.runtime.request(&[query]);
            products.runtime.wait(request).unwrap();
            let actual = products.lower.settled_definition(&root).unwrap();
            assert_eq!(actual.path, expected.path);
            assert_eq!(
                products.lower.work(&root).lowerings,
                0,
                "resident Lower replays the same ledger"
            );
            assert!(
                !products
                    .front
                    .work_snapshot()
                    .iter()
                    .any(|(p, kind, count)| p == &root && *kind == QueryKind::Modes && *count != 0)
            );
            assert_eq!(
                products
                    .lower
                    .settled_constant(&hidden, &[2])
                    .unwrap()
                    .root
                    .kind,
                hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(4))
            );
            assert!(
                products.lower.settled_constant(&hidden, &[3]).is_err(),
                "unselected size stays absent"
            );
        }
        products.runtime.shutdown();
        drop(products);
        drop(fresh);
        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
            store::ResultStore::put(&*store, &key, &bytes);
        }
        let mut remote = signature_session(copied, source, false);
        remote.adapters.modes = None;
        remote.adapters.lowering = None;
        let accepted = match StagedProducts::capture(&mut remote, 1) {
            Err(_) => false,
            Ok(mut products) => {
                let request = products
                    .runtime
                    .request(&[products.lower.query(&root).unwrap()]);
                let accepted = products.runtime.wait(request).is_ok();
                products.runtime.shutdown();
                accepted
            }
        };
        assert!(!accepted, "producer ledger cannot transfer V completion");
        let mut oracle =
            signature_session(store, source, true).with_verify_store(store::VerifyStore::Recompute);
        oracle
            .register_staged_definition(&definition("scaled"))
            .unwrap();
        oracle.lower_to_tir(&root).unwrap();
    });
}

#[test]
fn ordinary_staged_registrations_restore_from_heads_without_syntax_or_execution_authority() {
    with_compiler_stack(|| {
        use crate::staged_product_fixture::StagedProducts;
        let source = "fn value[n:size]()->u64{comptime (n*n)}\n";
        let owner = definition("value");
        let store = Arc::new(store::MemStore::default());
        let mut writer = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = writer
            .sources
            .add("staged-heads.hir", source.into())
            .unwrap();
        writer.index_source(file, &default_context()).unwrap();
        let mut products = StagedProducts::capture(&mut writer, 1).unwrap();
        let request = products
            .runtime
            .request(&[products.lower.query(&owner).unwrap()]);
        products.runtime.wait(request).unwrap();
        let lowered = products.lower.settled_definition(&owner).unwrap();
        let hidden = lowered
            .bodies
            .iter()
            .flat_map(|b| &b.rvalues)
            .find_map(|v| match v {
                hirsel_tir::Rvalue::ReadConstant {
                    definition,
                    arguments,
                } if !arguments.is_empty() => Some(definition.clone()),
                _ => None,
            })
            .unwrap();
        for n in [2, 3] {
            let query = products.lower.staged_query(&hidden, &[n]).unwrap();
            let request = products.runtime.request(&[query]);
            products.runtime.wait(request).unwrap();
        }
        products.runtime.shutdown();
        drop(products);
        drop(writer);
        let open = |store: Arc<dyn store::ResultStore>| {
            let mut reader = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store);
            reader.sources.add("padding.hir", "\n".into()).unwrap();
            let file = reader
                .sources
                .add("staged-heads.hir", source.into())
                .unwrap();
            reader.index_source(file, &default_context()).unwrap();
            reader.adapters.lexer = None;
            reader.adapters.parser = None;
            reader.adapters.signature = None;
            reader.adapters.modes = None;
            reader.adapters.lowering = None;
            reader
        };
        let mut reader = open(store.clone());
        let mut products = StagedProducts::capture(&mut reader, 1).unwrap();
        let request = products
            .runtime
            .request(&[products.lower.query(&owner).unwrap()]);
        products.runtime.wait(request).unwrap();
        for n in [2, 3] {
            let query = products.lower.staged_query(&hidden, &[n]).unwrap();
            let request = products.runtime.request(&[query]);
            products.runtime.wait(request).unwrap();
            assert_eq!(
                products
                    .lower
                    .settled_constant(&hidden, &[n])
                    .unwrap()
                    .root
                    .kind,
                hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(u128::from(n * n)))
            );
        }
        let query = products.lower.staged_query(&hidden, &[4]).unwrap();
        let request = products.runtime.request(&[query]);
        assert!(products.runtime.wait(request).is_err());
        assert!(products.lower.staged_query(&hidden, &[2, 3]).is_err());
        assert_eq!(reader.work_stats().parsed_bytes, 0);
        assert!(
            !products
                .front
                .work_snapshot()
                .iter()
                .any(|(p, kind, count)| p == &owner && *kind == QueryKind::Modes && *count != 0)
        );
        products.runtime.shutdown();
        drop(products);
        drop(reader);
        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
            store::ResultStore::put(&*store, &key, &bytes);
        }
        let mut untrusted = open(copied);
        let mut products = StagedProducts::capture(&mut untrusted, 1).unwrap();
        let query = products.lower.staged_query(&hidden, &[2]).unwrap();
        let request = products.runtime.request(&[query]);
        assert!(
            products.runtime.wait(request).is_err(),
            "ordinary registration carries no execution completion"
        );
        products.runtime.shutdown();
        drop(products);
        drop(untrusted);
        let mut oracle = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store)
            .with_verify_store(store::VerifyStore::Recompute);
        let file = oracle
            .sources
            .add("staged-heads.hir", source.into())
            .unwrap();
        oracle.index_source(file, &default_context()).unwrap();
        oracle.register_staged_definition(&owner).unwrap();
        oracle.lower_to_tir(&owner).unwrap();
        assert_eq!(
            oracle.constant_instance(&hidden, &[2]).unwrap().root.kind,
            hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(4))
        );
    });
}

#[test]
fn ordinary_read_only_executed_helper_keeps_its_check_product() {
    with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let dir = scratch.path();
        let manifest = dir.join("hirsel.json");
        std::fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"library","version":"1"},"sources":[{"path":"api.hir","module":[]}],"dependencies":[]}).to_string()).unwrap();
        std::fs::write(dir.join("api.hir"), "fn alphabet_bytes(x:u32)->u32{x+1u32}\nfn decode_words()->u32{alphabet_bytes(6u32)}\npub const DECODE:u32=comptime decode_words()\n").unwrap();
        let products = dir.join("products");
        let compiler = COMPILER.clone().with_config(CompileConfig {
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        });
        compiler.precompute_package(&manifest, &products).unwrap();
        let mut reader = compiler
            .with_config(CompileConfig {
                store: Arc::new(
                    store::DirStore::new(dir.join("reader")).with_read_only_root(products),
                ),
                jobs: hirsel_intern::jobs::Jobs::new(1),
                ..CompileConfig::for_memory_cap(None)
            })
            .load_check_package(&manifest)
            .unwrap();
        let mut root = reader.entry.clone();
        root.0.name = "DECODE".into();
        let (front, lower, value) = constant_product(&mut reader.engine, &root).unwrap();
        assert_eq!(
            value.root.kind,
            hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(7))
        );
        for name in ["alphabet_bytes", "decode_words", "DECODE"] {
            root.0.name = name.into();
            reader.engine.semantic_program_revision(&root).unwrap();
            let budget = ProcessBudget::new(1 << 30);
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::new(1),
                budget.reserve(512 << 20).unwrap(),
                crate::staged_product_fixture::errors(),
            )
            .unwrap();
            runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
            runtime
                .register_local(FRONT_LOWER_KIND, lower.factory())
                .unwrap();
            runtime
                .register_local(FRONT_EVALUATE_KIND, lower.factory())
                .unwrap();
            let request = runtime.request(&[lower.query(&root).unwrap()]);
            runtime.wait(request).unwrap();
            lower.settled_definition(&root).unwrap();
            runtime.shutdown();
            for phase in [QueryKind::Check, QueryKind::Modes] {
                assert_eq!(
                    reader.engine.recompute_count(&root, phase),
                    0,
                    "scalar semantic reader {root}: {phase:?}"
                );
                assert_eq!(front.recompute_count(&root, phase), 0, "{root}: {phase:?}");
            }
        }
    });
}

fn constant_product(
    engine: &mut QueryEngine,
    path: &DefPath,
) -> Result<
    (
        Arc<FrontBodyQueries>,
        Arc<FrontLowerQueries>,
        hirsel_tir::MaterializedRef,
    ),
    QueryError,
> {
    let mut products = crate::staged_product_fixture::StagedProducts::capture(engine, 1)?;
    let query = products
        .lower
        .evaluate_query(path)
        .ok_or(QueryError::NotImplemented)?;
    let request = products.runtime.request(&[query]);
    let result = products.runtime.wait(request).map_err(|reference| {
        products
            .lower
            .failure(reference)
            .unwrap_or(QueryError::NotImplemented)
    });
    products.runtime.shutdown();
    result?;
    let value = products.lower.settled_constant(path, &[])?;
    Ok((products.front, products.lower, value))
}
