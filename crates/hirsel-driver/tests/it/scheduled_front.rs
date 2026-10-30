use hirsel_driver::*;
use hirsel_engine::{
    runtime::*,
    store::{Domain, MemStore, ResultStore},
    *,
};
use std::sync::Arc;

pub(super) fn errors() -> RuntimeErrors {
    let reference = |text: &[u8]| {
        Domain {
            kind: 0x4600_00ff,
            schema: 1,
        }
        .reference(text)
        .unwrap()
    };
    RuntimeErrors {
        resource: reference(b"resource"),
        cycle: reference(b"cycle"),
        cancelled: reference(b"cancelled"),
        protocol: reference(b"protocol"),
        panicked: reference(b"panic"),
    }
}
pub(super) fn imported_fixture(
    store: &dyn ResultStore,
    address: hirsel_engine::store::Address,
    action: hirsel_engine::store::Action,
    reference: hirsel_engine::store::Reference,
) -> ProductRef {
    let budget = ProcessBudget::new(64 << 20);
    let mut allowance = budget.reserve(64 << 20).unwrap();
    let archive = ProductArchive::open(store, reference, &mut allowance).unwrap();
    ProductRef::import_archive(address, action, archive)
}

pub(super) fn synthetic_fixture(
    address: hirsel_engine::store::Address,
    action: hirsel_engine::store::Action,
    domain: Domain,
    bytes: &[u8],
) -> ProductRef {
    let store = MemStore::default();
    let reference = store.put_content(domain, bytes).unwrap();
    imported_fixture(&store, address, action, reference)
}

#[test]
fn admitted_real_front_tasks_match_scalar_modes_and_complete_reads() {
    hirsel_engine::with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(
            &path,
            "fn left()->u8{1u8}\nfn right()->u8{left()}\npub fn main()->u8{right()}\n",
        )
        .unwrap();
        for jobs in [1, 2] {
            let store = Arc::new(MemStore::default());
            let config = CompileConfig {
                store: store.clone(),
                jobs: hirsel_intern::jobs::Jobs::new(jobs),
                ..Default::default()
            };
            let mut program = COMPILER
                .with_config(config)
                .with_owned_engines()
                .load_program(&path)
                .unwrap();
            let mut definitions = vec![];
            for name in ["left", "right", "main"] {
                let mut path = program.entry.clone();
                path.0.name = name.into();
                definitions.push(path);
            }
            let budget = ProcessBudget::new(1 << 30);
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::new(jobs),
                budget.reserve(512 << 20).unwrap(),
                errors(),
            )
            .unwrap();
            let queries = program
                .engine
                .front_body_queries(
                    &definitions,
                    MemoryWeight {
                        retained: 1 << 20,
                        scratch: 8 << 20,
                        stack: 0,
                    },
                    budget.reserve(8 << 20).unwrap(),
                    errors(),
                )
                .unwrap();
            let roots: Vec<_> = definitions
                .iter()
                .map(|path| queries.query(path).unwrap())
                .collect();
            runtime
                .register(FRONT_BODY_KIND, queries.factory())
                .unwrap();
            let request = runtime.request(&roots);
            let outputs = runtime.wait(request).unwrap();
            assert_eq!(outputs.len(), 3);
            assert_eq!(runtime.stats().claims, 3);
            assert_eq!(runtime.stats().polls, 3);
            for product in &outputs {
                assert!(product.archive(ArchiveAdmission::Retained).is_err());
            }
            let bodies: Vec<_> = definitions
                .iter()
                .map(|definition| queries.body_query(definition).unwrap())
                .collect();
            let request = runtime.request(&bodies);
            let bodies = runtime.wait(request).unwrap();
            let mut crossed = bodies[1].clone();
            crossed.address = bodies[0].address;
            crossed.action = bodies[0].action;
            assert!(queries.outcome(Ok(crossed.clone())).is_none());
            assert!(queries.checked(Ok(crossed)).is_none());
            let mut crossed = outputs[1].clone();
            crossed.address = outputs[0].address;
            crossed.action = outputs[0].action;
            assert!(queries.checked(Ok(crossed)).is_none());
            for ((definition, product), body) in definitions.iter().zip(outputs).zip(bodies) {
                assert!(body.archive(ArchiveAdmission::Retained).is_err());
                let held = queries.outcome(Ok(body)).unwrap();
                let actual = &held.outcome;
                let checked = queries.checked(Ok(product.clone())).unwrap();
                assert_eq!(
                    checked.summary.summary,
                    actual
                        .summary
                        .as_ref()
                        .unwrap()
                        .checked
                        .as_ref()
                        .unwrap()
                        .summary
                );
                assert!(product.archive(ArchiveAdmission::Retained).is_err());
                let scalar = program.engine.mode_definition(definition).unwrap();
                assert_eq!(
                    actual.result.as_ref().unwrap().diagnostics,
                    scalar.diagnostics
                );
                assert_eq!(
                    actual.result.as_ref().unwrap().dependencies,
                    scalar.dependencies
                );
                let expected = program
                    .engine
                    .memo_reads(definition, QueryKind::Modes)
                    .unwrap();
                assert_eq!(
                    &*actual.reads, expected,
                    "complete front reads for {definition}, jobs={jobs}"
                );
                let published = actual.summary.as_ref().unwrap();
                let checked = queries.checked(Ok(product.clone())).unwrap();
                assert!(
                    Arc::ptr_eq(published.checked.as_ref().unwrap(), &checked.summary),
                    "the published and checked views retain the same immutable summary allocation"
                );
                let persisted = store.lookup_product(product.address).unwrap().reference;
                assert!(store.get_content(persisted).is_some());
            }
            runtime.shutdown();
        }
    });
}

#[test]
fn admitted_lower_saved_work_and_native_metadata_survive_dependency_suspension() {
    hirsel_engine::with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(&path, "const A:u8=comptime 1u8;\nconst B:u8=comptime A;\nfn helper()->u8{B}\npub fn main()->u8{comptime helper()}\n").unwrap();
        for jobs in [1, 2] {
            let store = Arc::new(MemStore::default());
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store: store.clone(),
                    jobs: hirsel_intern::jobs::Jobs::new(jobs),
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&path)
                .unwrap();
            let definitions: Vec<_> = ["A", "B", "helper", "main"]
                .iter()
                .map(|name| {
                    let mut path = program.entry.clone();
                    path.0.name = (*name).into();
                    path
                })
                .collect();
            let budget = ProcessBudget::new(1 << 30);
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::new(jobs),
                budget.reserve(512 << 20).unwrap(),
                errors(),
            )
            .unwrap();
            let weight = MemoryWeight {
                retained: 2 << 20,
                scratch: 8 << 20,
                stack: 0,
            };
            let front = program
                .engine
                .front_body_queries(
                    &definitions,
                    weight,
                    budget.reserve(16 << 20).unwrap(),
                    errors(),
                )
                .unwrap();
            let lower = program
                .engine
                .front_lower_queries(
                    &definitions,
                    front.clone(),
                    weight,
                    32,
                    budget.reserve(32 << 20).unwrap(),
                    errors(),
                )
                .unwrap();
            runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
            runtime
                .register_local(FRONT_LOWER_KIND, lower.factory())
                .unwrap();
            runtime
                .register_local(FRONT_EVALUATE_KIND, lower.factory())
                .unwrap();
            let query = lower.query(&program.entry).unwrap();
            let request = runtime.request(&[query]);
            let products = runtime
                .wait(request)
                .unwrap_or_else(|error| panic!("{:?}", lower.failure(error)));
            let metadata = lower.native_product((products[0]).clone()).unwrap();
            assert!(
                metadata.verified.is_some(),
                "preserve the actual opaque source V output"
            );
            for (path, lowerings, initializers) in [
                (&definitions[0], 0, 1),
                (&definitions[1], 0, 1),
                (&definitions[2], 1, 0),
                (&definitions[3], 1, 1),
            ] {
                let work = lower.work(path);
                assert_eq!(
                    (work.lowerings, work.initializers),
                    (lowerings, initializers),
                    "saved producer work for {path}, jobs={jobs}"
                );
                for kind in [QueryKind::Resolve, QueryKind::Check, QueryKind::Modes] {
                    assert_eq!(front.recompute_count(path, kind), 1);
                }
            }
            let phase_rows = front.memo_snapshot();
            for path in &definitions {
                for kind in [QueryKind::Resolve, QueryKind::Check, QueryKind::Modes] {
                    assert!(
                        phase_rows
                            .iter()
                            .any(|(key, _)| key.def.path() == path && key.kind == kind)
                    );
                }
            }
            let front_work = front.work_snapshot();
            assert_eq!(
                front_work.iter().map(|(_, _, count)| count).sum::<u64>(),
                12
            );
            assert_eq!(
                lower
                    .work_snapshot()
                    .iter()
                    .map(|(_, work)| work.initializers)
                    .sum::<u64>(),
                3
            );
            let lower_rows = lower.memo_snapshot().unwrap();
            assert!(
                lower_rows.iter().any(
                    |(key, _)| key.def.path() == &program.entry && key.kind == QueryKind::Lower
                )
            );
            let actual = lower.outcome((products[0]).clone()).unwrap();
            assert_eq!(
                lower_rows
                    .iter()
                    .find(
                        |(key, _)| key.def.path() == &program.entry && key.kind == QueryKind::Lower
                    )
                    .unwrap()
                    .1
                    .as_ref(),
                &actual.reads
            );
            let execution = actual
                .execution
                .as_ref()
                .expect("actual private execution evidence");
            assert!(
                execution.artifacts().len() >= 3,
                "includes main, A and B private artifacts"
            );
            let snapshot = program.engine.signature_snapshot();
            let cache = store.verifier().unwrap();
            assert!(
                cache
                    .execution(actual.execution_subject, |package| snapshot
                        .validation_input_digest(package))
                    .is_some()
            );
            assert!(
                cache
                    .execution(actual.execution_subject, |_| {
                        hirsel_syntax::ContentHash::of_bytes(b"changed private view")
                    })
                    .is_none()
            );
            let scalar = program.engine.lower_to_tir(&program.entry).unwrap();
            let records = NativeRecords::new(program.engine.types().clone(), store.clone());
            assert_eq!(
                records.encode(&program.engine, &actual.output.value),
                records.encode(&program.engine, &scalar.value)
            );
            let analyzed = program.engine.mode_definition(&program.entry).unwrap();
            let declared = lower.signature(&program.entry).unwrap();
            let expected = program
                .engine
                .prepare_comptime(&analyzed.value, &declared)
                .unwrap();
            assert!(!expected.materialized.is_empty());
            let expected_materialized = records
                .encode(&program.engine, &expected.materialized)
                .unwrap();
            assert_eq!(
                records
                    .encode(&program.engine, &actual.materialized)
                    .unwrap(),
                expected_materialized,
                "provider substitutions preserve scalar values and expression identities"
            );
            let detached_source = NativeRecordContext::capture(&program.engine);
            assert!(
                products[0].lower_value().is_some(),
                "ordinary Lower keeps a live owner"
            );
            let native = program
                .engine
                .native_original_context(&[program.entry.clone()])
                .unwrap()
                .reader([lower.native_product(products[0].clone()).unwrap()])
                .unwrap();
            assert!(native.read(&program.entry).is_some());
            let repeated = runtime.request(&[query]);
            assert_eq!(runtime.wait(repeated).unwrap(), products);
            assert_eq!(
                lower.work(&program.entry).initializers,
                1,
                "a second waiter executes no initializer again"
            );
            assert_eq!(
                runtime.stats().claims,
                8,
                "four P, two Lower and two Evaluate nodes"
            );
            assert!(
                runtime.stats().polls > runtime.stats().claims,
                "real dependency suspension occurred"
            );
            runtime.shutdown();
            drop(runtime);
            drop(program);
            let detached = lower.outcome((products[0]).clone()).unwrap();
            assert_eq!(
                records
                    .encode(&detached_source, &detached.materialized)
                    .unwrap(),
                expected_materialized,
                "the original substitution archive decodes after engine and runtime release"
            );
        }
    });
}

#[test]
fn admitted_lower_depth_matches_scalar_after_warm_constant_dependencies() {
    hirsel_engine::with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(&path,"const A:u8=comptime 1u8;\nconst B:u8=comptime A;\nconst C:u8=comptime B;\npub fn main()->u8{C}\n").unwrap();
        crate::image_fixture::core_manifest(&path, &directory.path().join("main.hir.json"));
        for jobs in [1, 2] {
            for limit in [0, 1, 2, 3, 4] {
                for warm in [false, true] {
                    let store = Arc::new(MemStore::default());
                    let mut program = COMPILER
                        .with_config(CompileConfig {
                            store: store.clone(),
                            jobs: hirsel_intern::jobs::Jobs::new(jobs),
                            ..Default::default()
                        })
                        .with_comptime_limits(hirsel_engine::ComptimeLimits {
                            call_depth: limit,
                            ..Default::default()
                        })
                        .with_owned_engines()
                        .load_program(&path)
                        .unwrap();
                    let definitions: Vec<_> = ["A", "B", "C", "main"]
                        .iter()
                        .map(|name| {
                            let mut path = program.entry.clone();
                            path.0.name = (*name).into();
                            path
                        })
                        .collect();
                    let budget = ProcessBudget::new(1 << 30);
                    let mut runtime = AdmittedRuntime::new(
                        hirsel_intern::jobs::Jobs::new(jobs),
                        budget.reserve(512 << 20).unwrap(),
                        errors(),
                    )
                    .unwrap();
                    let weight = MemoryWeight {
                        retained: 2 << 20,
                        scratch: 8 << 20,
                        stack: 0,
                    };
                    let front = program
                        .engine
                        .front_body_queries(
                            &definitions,
                            weight,
                            budget.reserve(16 << 20).unwrap(),
                            errors(),
                        )
                        .unwrap();
                    let lower = program
                        .engine
                        .front_lower_queries(
                            &definitions,
                            front.clone(),
                            weight,
                            32,
                            budget.reserve(32 << 20).unwrap(),
                            errors(),
                        )
                        .unwrap();
                    runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
                    runtime
                        .register_local(FRONT_LOWER_KIND, lower.factory())
                        .unwrap();
                    runtime
                        .register_local(FRONT_EVALUATE_KIND, lower.factory())
                        .unwrap();
                    if warm {
                        let request =
                            runtime.request(&[lower.evaluate_query(&definitions[1]).unwrap()]);
                        let _ = runtime.wait(request);
                    }
                    let request = runtime.request(&[lower.query(&program.entry).unwrap()]);
                    let actual = runtime
                        .wait(request)
                        .map(|_| ())
                        .map_err(|reference| lower.failure(reference).unwrap());
                    let expected = program.engine.lower_to_tir(&program.entry).map(|_| ());
                    assert_eq!(actual, expected, "jobs={jobs}, limit={limit}, warm={warm}");
                    runtime.shutdown();
                }
            }
        }
    });
}

#[test]
fn admitted_lower_reserves_outputs_only_for_demanded_definitions() {
    use hirsel_engine::*;
    // Retention is debited by each actual settled outcome, so the allowance
    // that holds exactly the entry's outcome is measured in a fresh probe.
    fn demand(extra: Option<u64>, spare_refused: bool) -> u64 {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(&path, "pub fn spare()->u8{1u8}\npub fn main()->u8{2u8}\n").unwrap();
        let store = Arc::new(hirsel_engine::store::MemStore::default());
        let mut program = COMPILER
            .with_config(CompileConfig {
                store,
                ..Default::default()
            })
            .with_owned_engines()
            .load_program(&path)
            .unwrap();
        let mut spare = program.entry.clone();
        spare.0.name = "spare".into();
        let definitions = vec![program.entry.clone(), spare.clone()];
        let budget = ProcessBudget::new(1 << 30);
        let weight = MemoryWeight {
            retained: 64 << 10,
            scratch: 8 << 20,
            stack: 0,
        };
        let errors = errors();
        let mut runtime = AdmittedRuntime::new(
            hirsel_intern::jobs::Jobs::new(1),
            budget.reserve(512 << 20).unwrap(),
            errors,
        )
        .unwrap();
        let front = program
            .engine
            .front_body_queries(
                &definitions,
                weight,
                budget.reserve(1 << 20).unwrap(),
                errors,
            )
            .unwrap();
        let inventory = FrontLowerQueries::input_bytes(&definitions).unwrap();
        let extra = extra.unwrap_or_else(|| FrontLowerQueries::output_bytes(weight).unwrap());
        let lower = program
            .engine
            .front_lower_queries(
                &definitions,
                front.clone(),
                weight,
                32,
                budget.reserve(inventory + extra).unwrap(),
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
        let request = runtime.request(&[lower.query(&program.entry).unwrap()]);
        let product = runtime.wait(request).unwrap()[0].clone();
        assert!(lower.outcome(product.clone()).is_some());
        assert_eq!(lower.work(&program.entry).lowerings, 1);
        assert_eq!(
            lower.work(&spare).lowerings,
            0,
            "an undemanded definition is never lowered"
        );
        let charged = lower.retained_bytes() - inventory;
        assert!(charged > 0 && charged <= extra);
        if spare_refused {
            let request = runtime.request(&[lower.query(&spare).unwrap()]);
            assert_eq!(runtime.wait(request).unwrap_err(), errors.resource);
            assert_eq!(
                lower.retained_bytes() - inventory,
                charged,
                "a refused outcome is not retained"
            );
        }
        runtime.shutdown();
        assert!(
            lower.outcome(product).is_some(),
            "held outputs survive worker shutdown with allowance owned"
        );
        charged
    }
    with_compiler_stack(|| {
        let charged = demand(None, false);
        assert_eq!(demand(Some(charged), true), charged);
    });
}

#[test]
fn admitted_front_store_hits_validate_signature_reads_before_materializing_bodies() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        let store = Arc::new(MemStore::default());
        for (source, main_hit, spare_hit, verify_cache) in [
            (
                "fn spare()->u8{1u8}\npub fn main(){let _=spare()}\n",
                false,
                false,
                false,
            ),
            (
                "fn spare()->u8{1u8}\npub fn main(){let _=spare()}\n",
                true,
                true,
                false,
            ),
            (
                "fn spare()->u8{2u8}\npub fn main(){let _=spare()}\n",
                true,
                false,
                false,
            ),
            (
                "fn spare()->u16{2u16}\npub fn main(){let _=spare()}\n",
                false,
                false,
                false,
            ),
            (
                "fn spare()->u16{2u16}\npub fn main(){let _=spare()}\n",
                false,
                false,
                true,
            ),
            (
                "fn spare()->u16{2u16}\npub fn main(){let _=spare()}\n",
                false,
                true,
                false,
            ),
        ] {
            std::fs::write(&path, source).unwrap();
            let mut program = COMPILER
                .with_config(hirsel_engine::CompileConfig {
                    store: store.clone(),
                    verify_store: if verify_cache {
                        hirsel_engine::store::VerifyStore::Recompute
                    } else {
                        hirsel_engine::store::VerifyStore::Off
                    },
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&path)
                .unwrap();
            let mut spare = program.entry.clone();
            spare.0.name = "spare".into();
            let definitions = vec![program.entry.clone(), spare.clone()];
            let budget = ProcessBudget::new(1 << 30);
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::new(1),
                budget.reserve(512 << 20).unwrap(),
                errors(),
            )
            .unwrap();
            let weight = MemoryWeight {
                retained: 1 << 20,
                scratch: 8 << 20,
                stack: 0,
            };
            let front = program
                .engine
                .front_body_queries(
                    &definitions,
                    weight,
                    budget.reserve(8 << 20).unwrap(),
                    errors(),
                )
                .unwrap();
            runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
            let request = runtime.request(
                &definitions
                    .iter()
                    .map(|path| front.query(path).unwrap())
                    .collect::<Vec<_>>(),
            );
            let products = runtime.wait(request).unwrap_or_else(|error| {
                panic!("{:?}", front.outcome(Err(error)).map(|o| o.outcome))
            });
            for (definition, hit) in [(&program.entry, main_hit), (&spare, spare_hit)] {
                for kind in [QueryKind::Resolve, QueryKind::Check, QueryKind::Modes] {
                    assert_eq!(
                        front.recompute_count(definition, kind),
                        u64::from(!hit),
                        "{definition} {kind:?}, verification={verify_cache}"
                    );
                }
            }
            assert_eq!(
                front.store_adoptions(),
                u64::from(main_hit) + u64::from(spare_hit)
            );
            for product in &products {
                assert!(product.summary_value().is_some());
                assert!(front.checked(Ok(product.clone())).is_some());
                assert!(product.archive(ArchiveAdmission::Retained).is_err());
            }
            runtime.shutdown();
            if verify_cache {
                let damaged = store
                    .lookup_product(products[0].address)
                    .unwrap()
                    .reference
                    .output
                    .0;
                let mut corrupted = 0;
                for (key, bytes) in store.take() {
                    if key.0 == damaged {
                        store.put(&key, &bytes[..bytes.len() / 2]);
                        corrupted += 1;
                    } else {
                        store.put(&key, &bytes);
                    }
                }
                assert_eq!(corrupted, 1, "damage only the completed main body record");
            }
        }
    });
}

#[test]
fn admitted_front_zero_hold_retains_only_archives_after_settlement() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(&path, "pub fn main()->u8{1u8}\n").unwrap();
        struct ContentOnly(MemStore);
        impl ResultStore for ContentOnly {
            fn get(
                &self,
                key: &hirsel_engine::store::EntryKey,
            ) -> Option<hirsel_engine::store::Bytes> {
                (key.2 == QueryKind::Interpret)
                    .then(|| self.0.get(key))
                    .flatten()
            }
            fn put(&self, key: &hirsel_engine::store::EntryKey, value: &[u8]) {
                if key.2 == QueryKind::Interpret {
                    self.0.put(key, value);
                }
            }
            fn name(&self) -> &'static str {
                "active-content-only-test"
            }
        }
        let store = Arc::new(ContentOnly(MemStore::default()));
        let mut program = COMPILER
            .with_config(CompileConfig {
                store,
                hold_bytes: 0,
                ..Default::default()
            })
            .with_owned_engines()
            .load_program(&path)
            .unwrap();
        let budget = ProcessBudget::new(1 << 30);
        let mut runtime = AdmittedRuntime::new(
            hirsel_intern::jobs::Jobs::ONE,
            budget.reserve(512 << 20).unwrap(),
            errors(),
        )
        .unwrap();
        let weight = MemoryWeight {
            retained: 1 << 20,
            scratch: 8 << 20,
            stack: 0,
        };
        let front = program
            .engine
            .front_body_queries(
                &[program.entry.clone()],
                weight,
                budget.reserve(8 << 20).unwrap(),
                errors(),
            )
            .unwrap();
        runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
        let query = front.query(&program.entry).unwrap();
        let request = runtime.request(&[query]);
        let product = runtime.wait(request).unwrap()[0].clone();
        assert_eq!(
            front.live_body_count(),
            0,
            "query owners retain no checked/analyzed body after settlement"
        );
        let body_query = front.body_query(&program.entry).unwrap();
        let request = runtime.request(&[body_query]);
        let body = runtime.wait(request).unwrap()[0].clone();
        let first = front.outcome(Ok(body.clone())).unwrap();
        assert_eq!(front.live_body_count(), 1);
        let second = front.outcome(Ok(body.clone())).unwrap();
        assert!(
            Arc::ptr_eq(
                first.outcome.result.as_ref().unwrap(),
                second.outcome.result.as_ref().unwrap()
            ),
            "concurrent decoders share an active body"
        );
        let with_body = front.retained_bytes();
        drop(second);
        drop(body);
        runtime.evict_to(0);
        assert!(first.outcome.result.is_ok());
        assert_eq!(front.retained_bytes(), with_body);
        drop(first);
        assert_eq!(front.live_body_count(), 0);
        assert!(front.retained_bytes() < with_body);
        let request = runtime.request(&[query]);
        let repeated = runtime.wait(request).unwrap()[0].clone();
        assert_eq!(product, repeated);
        assert_eq!(front.live_body_count(), 0);
        for kind in [QueryKind::Resolve, QueryKind::Check, QueryKind::Modes] {
            assert_eq!(
                front.recompute_count(&program.entry, kind),
                1,
                "archive demand does not repeat transformation"
            );
        }
        let request = runtime.request(&[body_query]);
        let body = runtime.wait(request).unwrap()[0].clone();
        let held = front.outcome(Ok(body.clone())).unwrap();
        assert!(held.outcome.result.is_ok());
        runtime.shutdown();
        drop(body);
        drop(front);
        assert!(
            budget.reserved() > 0,
            "held output retains admitted immutable owner after shutdown"
        );
        drop(held);
    });
}

#[test]
fn admitted_front_pressure_evicts_only_unpinned_settled_bodies_and_recomputes_equal() {
    with_compiler_stack(|| {
        struct Transport(MemStore);
        impl ResultStore for Transport {
            fn query_cache_enabled(&self) -> bool {
                false
            }
            fn get(
                &self,
                key: &hirsel_engine::store::EntryKey,
            ) -> Option<hirsel_engine::store::Bytes> {
                self.0.get(key)
            }
            fn put(&self, key: &hirsel_engine::store::EntryKey, value: &[u8]) {
                self.0.put(key, value);
            }
            fn name(&self) -> &'static str {
                "evictable-body-transport"
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(
            &path,
            "fn a()->u8{1u8}\nfn b()->u8{1u8}\npub fn main()->u8{0u8}\n",
        )
        .unwrap();
        let budget = ProcessBudget::new(1 << 30);
        let weight = MemoryWeight {
            retained: 1 << 20,
            scratch: 8 << 20,
            stack: 0,
        };
        let mut limit = 8 << 20;
        // First measure the complete metadata and one body through the same harness.
        for mode in 0..6 {
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store: Arc::new(Transport(MemStore::default())),
                    hold_bytes: 0,
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&path)
                .unwrap();
            let definitions = ["a", "b"].map(|name| {
                let mut definition = program.entry.clone();
                definition.0.name = name.into();
                definition
            });
            let mut runtime = AdmittedRuntime::with_query_capacity(
                hirsel_intern::jobs::Jobs::ONE,
                budget
                    .reserve(hirsel_intern::jobs::COMPILER_STACK_BYTES as u64 + 2 * weight.scratch)
                    .unwrap(),
                errors(),
                6,
            )
            .unwrap();
            let front = program
                .engine
                .front_body_queries(
                    &definitions,
                    weight,
                    budget.reserve(limit).unwrap(),
                    errors(),
                )
                .unwrap();
            runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
            let bodies = definitions
                .each_ref()
                .map(|definition| front.body_query(definition).unwrap());
            let root = QueryId {
                kind: 0x255,
                address: bodies[0].address,
            };
            let middle = QueryId {
                kind: 0x256,
                address: bodies[0].address,
            };
            runtime
                .register(
                    middle.kind,
                    Arc::new(DependencyFactory(vec![bodies[0]], middle)),
                )
                .unwrap();
            let dependencies = if mode == 5 {
                vec![middle, bodies[1]]
            } else {
                bodies.to_vec()
            };
            runtime
                .register(root.kind, Arc::new(DependencyFactory(dependencies, root)))
                .unwrap();
            let summaries = definitions
                .each_ref()
                .map(|definition| front.query(definition).unwrap());
            let request = runtime.request(&summaries);
            let publications = runtime.wait(request).unwrap();
            let metadata = front.retained_bytes();
            let request = runtime.request(&[bodies[0]]);
            let a = runtime.wait(request).unwrap().pop().unwrap();
            let mut scratch = budget.reserve(2 << 20).unwrap();
            let canonical = a
                .analyzed_value()
                .unwrap()
                .archive(&mut scratch, 1 << 20)
                .unwrap();
            let action = a.action;
            let reads = a.analyzed_value().unwrap().reads().as_ref().clone();
            if mode == 0 {
                limit = front.retained_bytes();
                assert!(limit > metadata);
                continue;
            }
            assert_eq!(
                front.retained_bytes(),
                limit,
                "partition fits exactly one body"
            );
            let pin = (mode == 2).then(|| front.outcome(Ok(a.clone())).unwrap());
            drop(a);
            struct DependencyFactory(Vec<QueryId>, QueryId);
            struct DependencyTask(Vec<QueryId>, QueryId);
            impl QueryFactory for DependencyFactory {
                fn create(
                    &self,
                    _: QueryId,
                ) -> Result<Box<dyn QueryTask>, hirsel_engine::store::Reference> {
                    Ok(Box::new(DependencyTask(self.0.clone(), self.1)))
                }
            }
            impl QueryTask for DependencyTask {
                fn id(&self) -> QueryId {
                    self.1
                }
                fn weight(&self) -> MemoryWeight {
                    MemoryWeight::default()
                }
                fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
                    for dependency in self.0.iter().copied() {
                        match cx.read(dependency) {
                            std::task::Poll::Pending => {
                                return QueryPoll::Waiting(vec![dependency]);
                            }
                            std::task::Poll::Ready(Err(error)) => return QueryPoll::Failed(error),
                            std::task::Poll::Ready(Ok(_)) => {}
                        }
                    }
                    QueryPoll::Complete(synthetic_fixture(
                        self.1.address,
                        hirsel_engine::store::Action(hirsel_syntax::ContentHash::of_bytes(
                            b"settled-consumer",
                        )),
                        Domain {
                            kind: 0x255,
                            schema: 1,
                        },
                        b"settled",
                    ))
                }
            }
            let request = if mode == 3 || mode == 5 {
                runtime.request(&[root])
            } else if mode == 4 {
                runtime.request(&bodies)
            } else {
                runtime.request(&[bodies[1]])
            };
            let b = runtime.wait(request);
            if mode == 3 || mode == 4 {
                assert_eq!(b, Err(errors().resource));
                assert_eq!(runtime.stats().body_evictions, 0);
                assert_eq!(front.live_body_count(), 1);
                continue;
            }
            if let Some(pin) = pin {
                assert_eq!(b, Err(errors().resource));
                assert_eq!(runtime.stats().body_evictions, 0);
                assert!(pin.outcome.result.is_ok());
                assert_eq!(front.live_body_count(), 1);
            } else {
                drop(b.unwrap());
                assert_eq!(runtime.stats().body_evictions, 1);
                assert_eq!(front.live_body_count(), 1);
                let request = runtime.request(&[bodies[0]]);
                let a = runtime.wait(request).unwrap().pop().unwrap();
                assert_eq!(runtime.stats().body_evictions, 2);
                assert_eq!(a.action, action);
                let recomputed = a
                    .analyzed_value()
                    .unwrap()
                    .archive(&mut scratch, 1 << 20)
                    .unwrap();
                assert_eq!(canonical.reference(), recomputed.reference());
                assert_eq!(canonical.bytes(), recomputed.bytes());
                assert_eq!(a.analyzed_value().unwrap().reads().as_ref(), &reads);
                assert_eq!(front.recompute_count(&definitions[0], QueryKind::Modes), 3);
                let request = runtime.request(&[summaries[0]]);
                assert_eq!(
                    runtime.wait(request).unwrap()[0],
                    publications[0],
                    "body recomputation preserves the original settled Summary owner"
                );
                assert_eq!(front.retained_bytes(), limit);
            }
        }
    });
}

#[test]
fn admitted_storage_recipe_waits_for_checked_body_and_matches_scalar_adapter() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(
            &path,
            "fn helper(x:u8)->u8{x}\npub fn main()->u8{helper(1u8)}\n",
        )
        .unwrap();
        for jobs in [1, 2] {
            let store = Arc::new(MemStore::default());
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store: store.clone(),
                    hold_bytes: 0,
                    jobs: hirsel_intern::jobs::Jobs::new(jobs),
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&path)
                .unwrap();
            let budget = ProcessBudget::new(1 << 30);
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::new(jobs),
                budget.reserve(512 << 20).unwrap(),
                errors(),
            )
            .unwrap();
            let weight = MemoryWeight {
                retained: 1 << 20,
                scratch: 8 << 20,
                stack: 0,
            };
            let front = program
                .engine
                .front_body_queries(
                    &[program.entry.clone()],
                    weight,
                    budget.reserve(8 << 20).unwrap(),
                    errors(),
                )
                .unwrap();
            let storage = program
                .engine
                .front_storage_queries(
                    front.clone(),
                    weight,
                    budget.reserve(8 << 20).unwrap(),
                    errors(),
                )
                .unwrap();
            runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
            runtime
                .register(FRONT_STORAGE_KIND, storage.factory())
                .unwrap();
            let query = storage.query(&program.entry).unwrap();
            let request = runtime.request(&[query]);
            let product = runtime
                .wait(request)
                .unwrap_or_else(|error| panic!("{:?}", storage.failure(error)))[0]
                .clone();
            let actual = storage.outcome(product.clone()).unwrap();
            assert_eq!(storage.executions(), 1);
            let analyzed = program.engine.mode_definition(&program.entry).unwrap();
            let snapshot = program.engine.signature_snapshot();
            let signature = snapshot
                .signature(hirsel_syntax::DefId::intern(&program.entry))
                .unwrap()
                .unwrap();
            let mut reads = hirsel_engine::ReadsBuilder::new();
            let mut provider = hirsel_engine::FrozenProvider::for_body(
                &snapshot,
                program.entry.0.package.clone(),
                &mut reads,
            );
            let expected = ModeAdapter
                .storage(
                    program.engine.types(),
                    &hirsel_types::TypeMemo::new(),
                    &analyzed.value,
                    signature,
                    &mut provider,
                )
                .unwrap();
            let records = NativeRecords::new(program.engine.types().clone(), store.clone());
            assert_eq!(
                records.encode(&program.engine, &actual.output.value),
                records.encode(&program.engine, &expected.value)
            );
            let expected_reads = reads.finish().unwrap();
            for (key, value) in expected_reads.iter() {
                assert!(
                    actual.reads.iter().any(
                        |(actual_key, actual_value)| actual_key == key && actual_value == value
                    ),
                    "missing actual storage signature read"
                );
            }
            let repeated = runtime.request(&[query]);
            assert_eq!(runtime.wait(repeated).unwrap(), vec![product.clone()]);
            assert_eq!(
                storage.executions(),
                1,
                "second consumer performs no storage transformation"
            );
            assert_eq!(
                runtime.stats().claims,
                2,
                "storage and its actual P dependency only"
            );
            drop(analyzed);
            runtime.evict_to(0);
            assert_eq!(
                front.live_body_count(),
                0,
                "storage equations retain no analyzed body"
            );
            runtime.shutdown();
        }
    });
}

#[test]
fn frozen_checked_runtime_matches_declared_heap_facts_without_body_queries() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(&path, "pub fn main(){}\n").unwrap();
        let mut program = COMPILER
            .with_config(CompileConfig {
                store: Arc::new(MemStore::default()),
                hold_bytes: 0,
                ..Default::default()
            })
            .with_owned_engines()
            .load_program(&path)
            .unwrap();
        let snapshot = program.engine.signature_snapshot();
        let before = program.engine.memo_snapshot();
        let actual = snapshot
            .checked_runtime(Some(&hirsel_syntax::DefPath::core(
                hirsel_interp::library::FORGET_OPERATION,
            )))
            .unwrap();
        assert_eq!(
            program.engine.memo_snapshot(),
            before,
            "frozen H declarations perform no body query"
        );
        let expected = program
            .engine
            .checked_runtime_types()
            .unwrap()
            .checked
            .unwrap();
        assert_eq!(actual.noalloc, expected.noalloc);
        assert_eq!(actual.noalloc_drop, expected.noalloc_drop);
        assert_eq!(actual.flat, expected.flat);
        assert_eq!(actual.arena_with, expected.arena_with);
        assert_eq!(
            actual
                .operations
                .iter()
                .map(|(path, signature)| (path, signature.fingerprint))
                .collect::<Vec<_>>(),
            expected
                .operations
                .iter()
                .map(|(path, signature)| (path, signature.fingerprint))
                .collect::<Vec<_>>()
        );
    });
}

#[test]
fn admitted_front_actual_output_debit_refuses_insufficient_producer_allowance() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(&path, "pub fn main()->u8{1u8}\n").unwrap();
        let store = Arc::new(MemStore::default());
        for extra in [1, 8 << 20] {
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store: store.clone(),
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&path)
                .unwrap();
            let definitions = vec![program.entry.clone()];
            let footprint = program.engine.front_input_footprint(&definitions).unwrap();
            assert!(footprint.source_headers > 0);
            let budget = ProcessBudget::new(1 << 30);
            let weight = MemoryWeight {
                retained: 64 << 10,
                scratch: 8 << 20,
                stack: 0,
            };
            let front = program
                .engine
                .front_body_queries(
                    &definitions,
                    weight,
                    budget.reserve(footprint.front_inputs + extra).unwrap(),
                    errors(),
                )
                .unwrap();
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::ONE,
                budget.reserve(512 << 20).unwrap(),
                errors(),
            )
            .unwrap();
            runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
            let request = runtime.request(&[front.query(&program.entry).unwrap()]);
            let result = runtime.wait(request);
            if extra == 1 {
                assert_eq!(result.unwrap_err(), errors().resource);
                assert_eq!(
                    front.recompute_count(&program.entry, QueryKind::Modes),
                    1,
                    "the real bounded producer ran; retention debit refused its publication"
                );
            } else {
                let products = result.unwrap();
                let output = front.checked(Ok(products[0].clone())).unwrap();
                assert_eq!(front.store_adoptions(), 1, "reuse remains real S0 adoption");
                drop(output);
            }
            runtime.shutdown();
            drop(front);
            assert_eq!(budget.reserved(), 0);
        }
    });
}
