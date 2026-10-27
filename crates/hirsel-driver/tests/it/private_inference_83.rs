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
            let reads = program
                .engine
                .memo_reads(&value, QueryKind::Signature)
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
