use hirsel_driver::*;
use hirsel_engine::{
    runtime::*,
    store::{MemStore, ResultStore},
    *,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[test]
fn admitted_native_definition_products_match_independent_scalar_products() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(&path,"pub type Secret{level:u64}\nconst A:u64=comptime 3u64;\nfn view(x:Secret)->u64{x.level}\npub fn main()->u64{A+view(Secret(level:4u64))}\n").unwrap();
        std::fs::write(&manifest,r#"{"schema":1,"package":{"name":"catalogue","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let mut previous_products = BTreeMap::new();
        for (jobs, verify) in [
            (1, Verify::On),
            (2, Verify::On),
            (1, Verify::Off),
            (2, Verify::Off),
        ] {
            let store = Arc::new(MemStore::default());
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store: store.clone(),
                    verify,
                    hold_bytes: 0,
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&manifest)
                .unwrap();
            let definitions = program.engine.front_source_roots();
            let budget = ProcessBudget::new(1 << 30);
            let errors = crate::scheduled_front::errors();
            let weight = MemoryWeight {
                retained: 64 << 10,
                scratch: 8 << 20,
                stack: 0,
            };
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::new(jobs),
                budget.reserve(512 << 20).unwrap(),
                errors,
            )
            .unwrap();
            let front = program
                .engine
                .front_body_queries(
                    &definitions,
                    weight,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let lower = program
                .engine
                .front_lower_queries(
                    &definitions,
                    front.clone(),
                    weight,
                    3 * definitions.len(),
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let native = program
                .engine
                .front_native_definition_queries(
                    &definitions,
                    front.clone(),
                    lower.clone(),
                    weight,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
            runtime
                .register_local(FRONT_LOWER_KIND, lower.factory())
                .unwrap();
            runtime
                .register_local(FRONT_EVALUATE_KIND, lower.factory())
                .unwrap();
            runtime
                .register(FRONT_NATIVE_DEFINITION_KIND, native.factory())
                .unwrap();
            let roots = definitions
                .iter()
                .map(|path| native.query(path).unwrap())
                .collect::<Vec<_>>();
            let request = runtime.request(&roots);
            let products = runtime
                .wait(request)
                .unwrap_or_else(|r| panic!("native product {r:?}: {:?}", native.failure(r)));
            let outputs = products
                .into_iter()
                .map(|p| native.outcome(p).unwrap())
                .collect::<Vec<_>>();
            for output in &outputs {
                let product = (output.product.product).clone().unwrap();
                let bytes = store
                    .get_content(
                        product
                            .archive(ArchiveAdmission::Retained)
                            .unwrap()
                            .reference(),
                    )
                    .unwrap();
                if let Some(previous) = previous_products.insert(
                    (
                        verify == Verify::On,
                        output.product.source.definition.clone(),
                    ),
                    bytes.to_vec(),
                ) {
                    assert_eq!(
                        previous.as_slice(),
                        &*bytes,
                        "unchanged native product, jobs={jobs}"
                    );
                }
            }
            assert!(
                definitions
                    .iter()
                    .any(|path| front.recompute_count(path, QueryKind::Modes) > 0)
            );
            assert_eq!(
                lower
                    .work_snapshot()
                    .iter()
                    .any(|(_, work)| work.validation_runs > 0),
                verify == Verify::On
            );
            assert_eq!(front.live_body_count(), 0);
            let actual_codec = NativeRecords::new(program.engine.types().clone(), store.clone());
            let anchors = NativeRecordContext::capture(&program.engine);
            let signatures = lower.signatures();
            let reader = native
                .original_context()
                .reader(
                    outputs
                        .iter()
                        .filter_map(|output| output.original.as_deref().cloned()),
                )
                .unwrap();
            let work_before_metadata = lower.work_snapshot();
            assert!(
                reader.retained_metadata_bytes()
                    >= std::mem::size_of::<NativeOriginalReader>() as u64
            );
            assert_eq!(
                lower.work_snapshot(),
                work_before_metadata,
                "reader metadata inspection performs no producer work"
            );
            runtime.shutdown();
            drop(runtime);
            let main_output = outputs
                .iter()
                .find(|o| o.product.source.definition == program.entry)
                .unwrap();
            let payload_bytes = store
                .get_content(
                    (main_output.product.product)
                        .clone()
                        .unwrap()
                        .archive(ArchiveAdmission::Retained)
                        .unwrap()
                        .reference(),
                )
                .unwrap();
            let payload = payload_bytes.len() as u64;
            // Reuse settled producers, but give a fresh native owner exactly
            // one payload's allowance. A byte less must refuse publication.
            for (available, accepts) in [(payload - 1, false), (payload, true)] {
                let inputs = program
                    .engine
                    .front_native_definition_inputs(&definitions, 16 << 20)
                    .unwrap();
                let grant = inputs.input_bytes() + available;
                let bounded = program
                    .engine
                    .front_native_definition_queries_with_inputs(
                        front.clone(),
                        lower.clone(),
                        inputs,
                        weight,
                        budget.reserve(grant).unwrap(),
                        errors,
                    )
                    .unwrap();
                assert_eq!(bounded.query(&program.entry), native.query(&program.entry));
                let mut bounded_runtime = AdmittedRuntime::new(
                    hirsel_intern::jobs::Jobs::new(jobs),
                    budget.reserve(512 << 20).unwrap(),
                    errors,
                )
                .unwrap();
                bounded_runtime
                    .register(FRONT_BODY_KIND, front.factory())
                    .unwrap();
                bounded_runtime
                    .register_local(FRONT_LOWER_KIND, lower.factory())
                    .unwrap();
                bounded_runtime
                    .register_local(FRONT_EVALUATE_KIND, lower.factory())
                    .unwrap();
                bounded_runtime
                    .register(FRONT_NATIVE_DEFINITION_KIND, bounded.factory())
                    .unwrap();
                let request = bounded_runtime.request(&[bounded.query(&program.entry).unwrap()]);
                let result = bounded_runtime.wait(request);
                if accepts {
                    let product = result.expect("one payload fits exactly")[0].clone();
                    assert_eq!(
                        store
                            .get_content(
                                product
                                    .archive(ArchiveAdmission::Retained)
                                    .unwrap()
                                    .reference()
                            )
                            .unwrap(),
                        payload_bytes
                    );
                    assert_eq!(
                        bounded.outcome(product).unwrap().product.source.definition,
                        program.entry
                    );
                } else {
                    assert_eq!(
                        result.unwrap_err(),
                        errors.resource,
                        "oversized payload remains refused"
                    );
                }
                bounded_runtime.shutdown();
            }
            drop(program);
            let main = reader
                .read(definitions.iter().find(|p| p.0.name == "main").unwrap())
                .expect("settled ordinary Lower remains usable after host drop");
            assert_eq!(main.definition().path.path().0.name, "main");
            let mut scalar = COMPILER
                .with_config(CompileConfig {
                    store: Arc::new(MemStore::default()),
                    verify,
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&manifest)
                .unwrap();
            let selected = definitions.iter().cloned().collect::<BTreeSet<_>>();
            let mut cache = NativeDiscoveryCache::new(
                scalar.engine.types().clone(),
                Arc::new(store::RequestStore::new(
                    Arc::new(MemStore::default()),
                    budget.reserve(8 << 20).unwrap(),
                )),
            );
            let scalar_codec =
                NativeRecords::new(scalar.engine.types().clone(), Arc::new(MemStore::default()));
            for (path, output) in definitions.iter().zip(&outputs) {
                let actual = &output.product;
                let expected = cache
                    .definition(&mut scalar.engine, path, &selected)
                    .unwrap();
                assert!(Arc::ptr_eq(
                    &actual.source.signature,
                    signatures.signature(DefId::intern(path)).unwrap().unwrap()
                ));
                fn observations(
                    codec: &NativeRecords,
                    owner: &impl NativeRecordSource,
                    product: &NativeDiscoveryProduct,
                ) -> Vec<u8> {
                    codec
                        .encode(
                            owner,
                            &(
                                &product.source.audit,
                                &product.source.materialized,
                                product.source.representation.as_ref().map(|r| {
                                    (
                                        &r.definition,
                                        r.representation_revision,
                                        &r.generics,
                                        r.private_shape,
                                        &r.layout_promises,
                                        r.resource,
                                        &r.finalizer,
                                        r.unsafe_ffi_package,
                                    )
                                }),
                                &product.source.constant,
                                &product.contracts,
                                &product.requirements,
                            ),
                        )
                        .unwrap()
                }
                let actual_bytes = observations(&actual_codec, &anchors, actual);
                let expected_bytes = observations(&scalar_codec, &scalar.engine, &expected);
                assert_eq!(
                    actual_bytes, expected_bytes,
                    "native observations for {path},jobs={jobs}"
                );
                assert_eq!(
                    actual.source.summary_identity, expected.source.summary_identity,
                    "summary identity for {path}"
                );
                if actual.contracts.is_some() {
                    let original = output.original.as_ref().expect("ordinary Lower descriptor");
                    assert_eq!(original.definition, *path);
                    assert_eq!(original.verified.is_some(), verify == Verify::On);
                    assert_eq!(
                        actual.source.lower.is_some(),
                        verify == Verify::On,
                        "actual V output for {path}"
                    );
                    if verify == Verify::Off {
                        assert_eq!(reader.input(path).unwrap().1, None);
                        let NativeOriginalValue::Live(live) = &original.body else {
                            panic!("selected live Lower")
                        };
                        assert!(Arc::ptr_eq(
                            reader.read(path).unwrap().definition(),
                            &live.output().value
                        ));
                    }
                }
            }
            drop(native);
            drop(front);
            drop(lower);
            assert!(budget.reserved() > 0, "held catalogue owns admission");
            drop(outputs);
            drop(reader);
            assert!(
                budget.reserved() > 0,
                "held ordinary body retains immutable source admission"
            );
            drop(main);
            drop(cache);
            assert_eq!(budget.reserved(), 0);
        }
    });
}
