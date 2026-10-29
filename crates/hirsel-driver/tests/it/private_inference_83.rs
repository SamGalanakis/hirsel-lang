use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;

#[test]
fn admitted_private_facts_reuse_unchanged_inputs_and_refuse_body_and_membership_edits() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"private_facts","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let store = Arc::new(MemStore::default());
        let budget = ProcessBudget::new(512 << 20);
        let source = "fn value(take data: unique Bytes)->Bytes { let _ = data[0]; data }\npub fn main()->u8 { 1u8 }\n";
        let check = |text: &str, expected: usize| {
            std::fs::write(&path, text).unwrap();
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store: store.clone(),
                    hold_bytes: 0,
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&manifest)
                .unwrap();
            let mut value = program.entry.clone();
            value.0.name = "value".into();
            let reservation = budget.reserve(256 << 20).unwrap();
            let reservation = program
                .engine
                .prepare_private_inference(reservation)
                .unwrap();
            assert_eq!(reservation.bytes(), 256 << 20);
            assert_eq!(
                program
                    .engine
                    .recompute_count(&value, QueryKind::PrivateInference),
                expected
            );
            for kind in [QueryKind::Resolve, QueryKind::Check, QueryKind::Modes] {
                assert_eq!(
                    program.engine.recompute_count(&value, kind),
                    0,
                    "inference enters S5: {kind:?}"
                );
            }
            hirsel_types::SignatureProvider::signature(
                &mut program.engine,
                hirsel_syntax::DefId::intern(&value),
            )
            .unwrap();
            let signature_reads = program
                .engine
                .memo_reads(&value, QueryKind::Signature)
                .unwrap();
            assert!(signature_reads.iter().any(
                |(key, _)| matches!(key, memo::ReadKey::PrivateSummary(id) if id.path() == &value)
            ));
            let reads = program
                .engine
                .memo_reads(&value, QueryKind::PrivateInference)
                .unwrap();
            assert!(
                reads
                    .iter()
                    .any(|(key, _)| matches!(key, memo::ReadKey::NameSet(..)))
            );
            assert!(reads.iter().any(
                |(key, _)| matches!(key, memo::ReadKey::CitedText(id) if id.path() == &value)
            ));
            drop(reservation);
            assert_eq!(budget.reserved(), 0);
        };
        check(source, 1);
        check(source, 0);
        check(&source.replace("{ 1u8 }", "{ 2u8 }"), 0);
        check(&source.replace("data[0]", "data[1]"), 1);
        check(&(source.to_owned() + "fn sibling()->u8 { 3u8 }\n"), 1);
    });
}

#[test]
fn private_facts_without_persistence_reuse_reads_after_an_unrelated_body_edit() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let context = default_context();
        let mut value = entry_path(&context);
        value.0.name = "value".into();
        let budget = ProcessBudget::new(512 << 20);
        for (source, count) in [
            (
                "fn value(take data: unique Bytes)->Bytes { let _ = data[0]; data }\npub fn main()->u8 { 1u8 }\n",
                1,
            ),
            (
                "fn value(take data: unique Bytes)->Bytes { let _ = data[0]; data }\npub fn main()->u8 { 2u8 }\n",
                1,
            ),
            (
                "fn value(take data: unique Bytes)->Bytes { let _ = data[1]; data }\npub fn main()->u8 { 2u8 }\n",
                2,
            ),
        ] {
            let file = engine.sources.add("local.hir", source.into()).unwrap();
            engine.parse_file(file, &context).unwrap();
            let reservation = engine
                .prepare_private_inference(budget.reserve(256 << 20).unwrap())
                .unwrap();
            assert_eq!(
                engine.recompute_count(&value, QueryKind::PrivateInference),
                count
            );
            assert_eq!(engine.recompute_count(&value, QueryKind::Check), 0);
            drop(reservation);
            assert_eq!(budget.reserved(), 0);
        }
    });
}

#[test]
fn verify_cache_waits_for_all_module_proofs_before_comparing_signatures() {
    with_compiler_stack(|| {
        let store = Arc::new(MemStore::default());
        let budget = ProcessBudget::new(512 << 20);
        for verify in [store::VerifyStore::Off, store::VerifyStore::Recompute] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone())
                .with_verify_store(verify);
            for module in ["a", "b"] {
                let mut context = default_context();
                context.modules = vec![module.into()];
                let file = engine
                    .sources
                    .add(
                        format!("{module}.hir"),
                        "fn value(take data: unique Bytes)->Bytes { let _ = data[0]; data }\npub fn main()->u8 { 1u8 }\n".into(),
                    )
                    .unwrap();
                engine.parse_file(file, &context).unwrap();
            }
            let reservation = engine
                .prepare_private_inference(budget.reserve(256 << 20).unwrap())
                .unwrap();
            drop(reservation);
            assert_eq!(budget.reserved(), 0);
        }
    });
}

#[test]
fn changing_the_root_does_not_reinfer_unchanged_dependency_modules() {
    with_compiler_stack(|| {
        let mut engine = crate::source_std::engine();
        let context = default_context();
        let std = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
            package: hirsel_syntax::PackageId {
                name: "std".into(),
                version: "1".into(),
            },
            modules: vec!["time".into()],
            name: "read_digits".into(),
        });
        let previous = engine.recompute_count(&std, QueryKind::PrivateInference);
        // Shared setup may restore the dependency's inferred proof with zero
        // local recomputations. Edits must retain that proof in either case.
        let header = engine.item_signature(&std).unwrap().value;
        let mut root = entry_path(&context);
        root.0.name = "root_helper".into();
        for number in [1u8, 2] {
            let source = format!(
                "fn root_helper()->u8{{{number}u8}}\npub fn root_entry()->u8{{root_helper()}}\n"
            );
            let file = engine.sources.add("root-proof.hir", source).unwrap();
            engine.parse_file(file, &context).unwrap();
            let budget = ProcessBudget::new(256 << 20);
            let root_before = engine.recompute_count(&root, QueryKind::PrivateInference);
            let reservation = engine
                .prepare_private_inference(budget.reserve(256 << 20).unwrap())
                .unwrap();
            drop(reservation);
            assert!(engine.recompute_count(&root, QueryKind::PrivateInference) > root_before);
            assert_eq!(
                engine.recompute_count(&std, QueryKind::PrivateInference),
                previous
            );
            assert!(Arc::ptr_eq(
                &header,
                &engine.item_signature(&std).unwrap().value
            ));
            assert_eq!(budget.reserved(), 0);
        }
    });
}

#[test]
fn generated_private_results_keep_their_inferred_unique_proof() {
    use hirsel_driver::{requests::*, resident::ObservedSession};
    with_compiler_stack(|| {
        let helpers = r#"
fn make()->Result[Body[fn()->Bytes uses {}],String] uses {} {
  Ok(codegen.body("fn()->Bytes uses {} {unique b\"x\"}"))
}
fn generated()->Bytes uses {} = comptime make()
"#;
        let main = "pub fn main()->unique Bytes uses {} {generated()}\n";
        for source in [format!("{helpers}{main}"), format!("{main}{helpers}")] {
            let directory = tempfile::tempdir().unwrap();
            let manifest = directory.path().join("hirsel.json");
            std::fs::write(directory.path().join("main.hir"), &source).unwrap();
            std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"generated_proof","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
            let config = || CompileConfig {
                store: Arc::new(MemStore::default()),
                jobs: hirsel_intern::jobs::Jobs::new(1),
                hold_bytes: 0,
                ..Default::default()
            };
            let mut scalar = COMPILER
                .with_config(config())
                .with_owned_engines()
                .load_program(&manifest)
                .unwrap();
            scalar.check_for_execution().unwrap();
            let config = config();
            let compiler = COMPILER.with_config(config.clone()).with_owned_engines();
            let budget = ProcessBudget::new(1 << 30);
            let selector = CompilerRequests::new(
                compiler.clone(),
                config.clone(),
                budget.clone(),
                RequestLimits {
                    runtime_bytes: 512 << 20,
                    snapshot_bytes: 128 << 20,
                    transport_bytes: 32 << 20,
                },
            );
            let limits = selector.input_limits();
            let mut session = ObservedSession::new(manifest, compiler, config, selector, limits, 3);
            {
                let checked = session
                    .request(CompileOperation::Check {
                        lowering: LoweringPolicy::Reachable,
                    })
                    .unwrap();
                let CompileResult::Check(checked) = &checked.result else {
                    panic!("check result")
                };
                assert_eq!(checked.completion, CheckCompletion::Complete);
                assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            }
            let changed = source.replace(
                "unique b\\\"x\\\"",
                "let value = unique b\\\"x\\\"; let saved = copy value; value",
            );
            assert_ne!(changed, source);
            std::fs::write(directory.path().join("main.hir"), changed).unwrap();
            let diagnostics = match session.request(CompileOperation::Check {
                lowering: LoweringPolicy::Reachable,
            }) {
                Ok(output) => {
                    let CompileResult::Check(checked) = &output.result else {
                        panic!("check result")
                    };
                    checked.diagnostics.clone()
                }
                Err(ToolError::Query {
                    error: QueryError::Diagnostics(diagnostics),
                    ..
                }) => diagnostics,
                Err(error) => panic!("{error:?}"),
            };
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.rule.as_str() == "MODE-2"),
                "{diagnostics:?}"
            );
        }
    });
}

#[test]
fn changed_private_result_proofs_invalidate_unchanged_callers() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let source = "const NAME:String=\"shared\"\nfn code()->String{\"fresh\"}\nfn need(take value:unique String){}\npub fn main(){need(code())}\n";
        let mut code = entry_path(&default_context());
        code.0.name = "code".into();
        let entry = entry_path(&default_context());
        let check = |engine: &mut QueryEngine, source: &str| {
            let file = engine
                .sources
                .add("private-result-edits.hir", source.into())
                .unwrap();
            check_progress(engine, file, &default_context()).unwrap()
        };
        let accepted = check(&mut engine, source);
        assert!(
            accepted.diagnostics.is_empty(),
            "{:?}",
            accepted.diagnostics
        );
        engine.lower_to_tir(&entry).unwrap();
        let fingerprint = engine.item_signature(&code).unwrap().value.fingerprint;
        let before = engine.recompute_count(&entry, QueryKind::Modes);

        let rejected = check(&mut engine, &source.replace("{\"fresh\"}", "{NAME}"));
        assert!(
            rejected
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str() == "MODE-2")
        );
        assert_ne!(
            engine.item_signature(&code).unwrap().value.fingerprint,
            fingerprint
        );
        assert!(engine.recompute_count(&entry, QueryKind::Modes) > before);

        let accepted = check(&mut engine, source);
        assert!(
            accepted.diagnostics.is_empty(),
            "{:?}",
            accepted.diagnostics
        );
        assert_eq!(
            engine.item_signature(&code).unwrap().value.fingerprint,
            fingerprint
        );
        engine.lower_to_tir(&entry).unwrap();
    });
}

#[test]
fn failed_private_inference_cannot_reuse_a_successful_signature_snapshot() {
    struct RefuseInference(Arc<dyn ModesPhase>);
    impl ModesPhase for RefuseInference {
        fn infers_private_unique(&self) -> bool {
            true
        }
        fn infer_unique(
            &self,
            _: &hirsel_types::TypeInterner,
            _: &hirsel_types::TypeMemo,
            _: &[(Arc<TypedBody>, hirsel_types::SignatureRef)],
            _: &mut dyn hirsel_types::SignatureProvider,
        ) -> Result<
            Vec<(hirsel_syntax::DefId, hirsel_types::UniqueInference)>,
            hirsel_types::QueryError,
        > {
            Err(hirsel_types::QueryError::NotImplemented)
        }
        fn check_modes(
            &self,
            types: &hirsel_types::TypeInterner,
            memo: &hirsel_types::TypeMemo,
            body: Arc<TypedBody>,
            signature: &hirsel_types::Signature,
            products: &mut PhaseProducts,
            signatures: &mut dyn hirsel_types::SignatureProvider,
        ) -> Result<hirsel_types::QueryOutput<AnalyzedBody>, hirsel_types::QueryError> {
            self.0
                .check_modes(types, memo, body, signature, products, signatures)
        }
    }
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let context = default_context();
        let entry = entry_path(&context);
        let mut helper = entry.clone();
        helper.0.name = "helper".into();
        let file = engine
            .sources
            .add(
                "failed-inference.hir",
                "fn helper()->u64{1u64}\npub fn main()->u64{helper()}".into(),
            )
            .unwrap();
        assert!(
            check_progress(&mut engine, file, &context)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        engine.lower_to_tir(&entry).unwrap();
        let original = engine.signature_snapshot();
        assert!(
            original
                .signature(hirsel_syntax::DefId::intern(&helper))
                .unwrap()
                .is_ok()
        );
        let modes = engine.adapters.modes.clone().unwrap();
        engine.adapters.modes = Some(Arc::new(RefuseInference(modes.clone())));
        let file = engine
            .sources
            .add(
                "failed-inference.hir",
                "fn helper()->u64{2u64}\npub fn main()->u64{helper()}".into(),
            )
            .unwrap();
        engine.parse_file(file, &context).unwrap();
        let budget = ProcessBudget::new(256 << 20);
        assert!(
            engine
                .prepare_private_inference(budget.reserve(256 << 20).unwrap())
                .is_err()
        );
        assert_eq!(budget.reserved(), 0);
        assert!(engine.item_signature(&helper).is_err());
        assert!(engine.mode_definition(&entry).is_err());
        assert!(engine.lower_to_tir(&entry).is_err());
        assert!(
            engine
                .signature_snapshot()
                .signature(hirsel_syntax::DefId::intern(&helper))
                .unwrap()
                .is_err()
        );
        engine.adapters.modes = Some(modes);
        engine
            .prepare_private_inference(budget.reserve(256 << 20).unwrap())
            .unwrap();
        assert!(engine.item_signature(&helper).is_ok());
        assert!(engine.mode_definition(&entry).is_ok());
        engine.lower_to_tir(&entry).unwrap();
        assert!(
            engine
                .signature_snapshot()
                .signature(hirsel_syntax::DefId::intern(&helper))
                .unwrap()
                .is_ok()
        );
    });
}

#[test]
fn conservative_headers_and_settled_proofs_have_distinct_verified_cache_inputs() {
    with_compiler_stack(|| {
        let source = "fn value()->String{\"fresh\"}\n";
        let store = Arc::new(MemStore::default());
        let context = default_context();
        let mut value = entry_path(&context);
        value.0.name = "value".into();
        let mut adapters = compiler_adapters();
        adapters.modes = None;
        let mut declaration =
            QueryEngine::new(default_environment(), adapters).with_result_store(store.clone());
        let file = declaration
            .sources
            .add("proof-inputs.hir", source.into())
            .unwrap();
        declaration.parse_file(file, &context).unwrap();
        assert!(
            declaration
                .item_signature(&value)
                .unwrap()
                .value
                .metadata
                .inferred_unique
                .results
                .is_empty()
        );

        let budget = ProcessBudget::new(256 << 20);
        let mut checked = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone())
            .with_verify_store(store::VerifyStore::Recompute);
        let file = checked
            .sources
            .add("proof-inputs.hir", source.into())
            .unwrap();
        checked.parse_file(file, &context).unwrap();
        let reservation = checked
            .prepare_private_inference(budget.reserve(256 << 20).unwrap())
            .unwrap();
        drop(reservation);
        assert_eq!(budget.reserved(), 0);
        let proof = checked.item_signature(&value).unwrap().value;
        assert!(!proof.metadata.inferred_unique.results.is_empty());

        let mut adapters = compiler_adapters();
        adapters.signature = None;
        adapters.modes = None;
        let mut restored =
            QueryEngine::new(default_environment(), adapters).with_result_store(store);
        let file = restored
            .sources
            .add("proof-inputs.hir", source.into())
            .unwrap();
        restored.parse_file(file, &context).unwrap();
        assert_eq!(
            restored.item_signature(&value).unwrap().value.fingerprint,
            proof.fingerprint
        );
        assert_eq!(restored.recompute_count(&value, QueryKind::Signature), 0);
    });
}

#[test]
fn nonretention_133_is_transitive_and_refuses_return_capture_and_recursive_assumptions() {
    with_compiler_stack(|| {
        let store = Arc::new(MemStore::default());
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let context = default_context();
        let source = r#"
type Bits { value: u64 }
fn leaf_bits(x: Bits) -> u64 { x.value }
fn bump(xs:List[u32],i:u64,y:u32)->u32 {xs[i]+y}
fn ensured(xs:List[u32],i:u64,y:u32)->u32 ensures result>=y {xs[i]+y}
fn required(xs:List[u32],i:u64,y:u32)->u32 requires i<2u64 {xs[i]+y}
fn returned_list(xs:List[u32])->List[u32] in xs {copy xs}
fn generic_discard[T](x:T) {}
fn generic_tuple_discard[T](x:(T,)) {}
fn relay_bits(x: Bits) -> u64 { leaf_bits(x) }
fn leaf(xs: Bytes) -> u8 { xs[0] }
fn relay(xs: Bytes) -> u8 { leaf(xs) }
fn returned(xs: Bytes) -> Bytes { copy xs }
fn retained(xs: Bytes) -> u8 { let saved = returned(xs); saved[0] }
fn captured(xs: Bytes) -> u8 { let f = fn() -> u8 { xs[0] }; f() }
fn recursive(xs: Bytes) -> u8 { recursive(xs) }
fn left(xs: Bytes) -> u8 { right(xs) }
fn right(xs: Bytes) -> u8 { left(xs) }
pub fn main() -> u8 { 0u8 }
"#;
        let file = engine
            .sources
            .add("nonretention.hir", source.into())
            .unwrap();
        engine.parse_file(file, &context).unwrap();
        let budget = ProcessBudget::new(512 << 20);
        let reservation = engine
            .prepare_private_inference(budget.reserve(256 << 20).unwrap())
            .unwrap();
        for (name, expected) in [
            ("leaf_bits", true),
            ("bump", true),
            ("ensured", false),
            ("required", false),
            ("returned_list", false),
            ("generic_discard", false),
            ("generic_tuple_discard", false),
            ("relay_bits", true),
            ("leaf", true),
            ("relay", true),
            ("returned", false),
            ("retained", false),
            ("captured", false),
            ("recursive", false),
            ("left", false),
            ("right", false),
        ] {
            let mut path = entry_path(&context);
            path.0.name = name.into();
            let signature = hirsel_types::SignatureProvider::signature(
                &mut engine,
                hirsel_syntax::DefId::intern(&path),
            )
            .unwrap();
            assert_eq!(
                signature.metadata.inferred_unique.nonretaining,
                if expected {
                    vec![hirsel_types::ParamId(0)]
                } else {
                    vec![]
                },
                "{name}"
            );
            assert!(engine.memo_reads(&path, QueryKind::Signature).unwrap().iter().any(|(key, _)| matches!(key, memo::ReadKey::PrivateSummary(id) if id.path() == &path)));
        }
        drop(reservation);
        let mut relay = entry_path(&context);
        relay.0.name = "relay_bits".into();
        // Plain nominal arguments are copied into a declared Borrow entry.
        engine.lower_to_tir(&relay).unwrap();
        for name in ["bump", "ensured", "required", "returned_list"] {
            let mut path = entry_path(&context);
            path.0.name = name.into();
            engine.lower_to_tir(&path).unwrap();
        }
        let mut restored =
            QueryEngine::new(default_environment(), compiler_adapters()).with_result_store(store);
        let file = restored
            .sources
            .add("nonretention.hir", source.into())
            .unwrap();
        restored.parse_file(file, &context).unwrap();
        let reservation = restored
            .prepare_private_inference(budget.reserve(256 << 20).unwrap())
            .unwrap();
        let signature = restored.item_signature(&relay).unwrap().value;
        assert_eq!(
            signature.metadata.inferred_unique.nonretaining,
            vec![hirsel_types::ParamId(0)]
        );
        assert_eq!(
            restored.recompute_count(&relay, QueryKind::PrivateInference),
            0
        );
        drop(reservation);
    });
}

#[test]
fn cross_module_declared_reads_reuse_proofs_before_private_headers_are_demanded() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"cross_private","version":"1"},"sources":[{"path":"a.hir","module":["a"]},{"path":"b.hir","module":["b"]}],"dependencies":[],"entry":{"module":["a"],"name":"main"}}"#).unwrap();
        let a = "fn left()->u8{b.right()}\nfn value()->u8{1u8}\npub fn main()->u8{left()}\n";
        std::fs::write(
            directory.path().join("b.hir"),
            "fn right()->u8{a.value()}\n",
        )
        .unwrap();
        let store = Arc::new(MemStore::default());
        let budget = ProcessBudget::new(512 << 20);
        for (source, a_count, b_count) in [
            (a.to_owned(), 1, 1),
            (a.to_owned(), 0, 0),
            (a.replace("1u8", "2u8"), 1, 0),
            (a.replace("value()->u8{1u8}", "value()->u64{1u64}"), 1, 1),
        ] {
            std::fs::write(directory.path().join("a.hir"), source).unwrap();
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store: store.clone(),
                    hold_bytes: 0,
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&manifest)
                .unwrap();
            let reservation = program
                .engine
                .prepare_private_inference(budget.reserve(256 << 20).unwrap())
                .unwrap();
            for (module, name, expected) in [
                ("a", "left", a_count),
                ("a", "value", a_count),
                ("b", "right", b_count),
            ] {
                let mut path = program.entry.clone();
                path.0.modules = vec![module.into()];
                path.0.name = name.into();
                assert_eq!(
                    program
                        .engine
                        .recompute_count(&path, QueryKind::PrivateInference),
                    expected,
                    "{path}"
                );
                if a_count == 0 && b_count == 0 {
                    assert_eq!(
                        program.engine.recompute_count(&path, QueryKind::Signature),
                        0,
                        "{path}"
                    );
                }
            }
            drop(reservation);
            assert_eq!(budget.reserved(), 0);
        }
    });
}

#[test]
fn standalone_generic_bootstrap_can_demand_another_private_generic_without_self_locking() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"private_bootstrap","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        std::fs::write(
            directory.path().join("main.hir"),
            r#"
type Value { x: u8 }
fn relay[T](s:TypeShape[T])->Result[Body[fn(T)->u8 uses {}],String] uses {} {
    let _ = s
    Ok(codegen.body("fn(v:Value)->u8 uses {} {7u8}"))
}
fn body[T](s:TypeShape[T])->Result[Body[fn(T)->u8 uses {}],String] uses {} {relay(s)}
fn generated[T](v:T)->u8 uses {} = comptime body(shape(T))
pub fn main()->u8 uses {} {generated(Value(x:7u8))}
"#,
        )
        .unwrap();
        let mut program = COMPILER
            .with_config(CompileConfig {
                store: Arc::new(MemStore::default()),
                jobs: hirsel_intern::jobs::Jobs::new(1),
                hold_bytes: 0,
                ..Default::default()
            })
            .with_owned_engines()
            .load_program(&manifest)
            .unwrap();
        match program.check_for_execution() {
            Ok(()) => {}
            Err(ToolError::Query {
                error: QueryError::Operational(failure),
                ..
            }) if failure.operation == "private generated inference"
                || failure.operation == "private inference admission" => {}
            Err(error) => panic!("unexpected bootstrap refusal: {error:?}"),
        }
    });
}
