//! Explicit CT-5 registration rounds, independent of the host bootstrap.
use hirsel_driver::COMPILER;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;

const SOURCE: &str = r#"
type Point { x: u8 }
type Box3 { x: u16 }
fn make[T](s: TypeShape[T]) -> Result[Body[fn(T)->u64 uses {}], String] uses {} {
  let info = type_shape.info(s)
  if info.name == "instance_front@1/Point" { Ok(codegen.body("fn(v:Point)->u64 uses {} {7u64}")) }
  else { Ok(codegen.body("fn(v:Box3)->u64 uses {} {7u64}")) }
}
fn generated[T](v: T)->u64 uses {} = comptime make(shape(T))
pub fn main()->u64 uses {} { generated(Point(x:1u8)) + generated(Box3(x:2u16)) + generated(Point(x:3u8)) }
"#;
#[test]
fn source_requests_install_nested_instance_views_for_check_run_and_native_ownership() {
    use hirsel_driver::{native, requests::*, resident::ObservedSession};
    with_compiler_stack(|| {
        for jobs in [1, 2] {
            let directory = tempfile::tempdir().unwrap();
            let manifest = directory.path().join("hirsel.json");
            let source = SOURCE.replace("{7u64}", "{ inner(v) }")
                + "\nfn inner[T](v:T)->u64 uses {} = comptime inner_make(shape(T))\n"
                + &SOURCE[SOURCE.find("fn make").unwrap()..SOURCE.find("fn generated").unwrap()]
                    .replace("make[T]", "inner_make[T]");
            let source = source + "\nfn zzz_unrelated()->u8{1u8}\nfn aaa_unrelated()->u8{2u8}\n";
            std::fs::write(directory.path().join("main.hir"), source).unwrap();
            std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"instance_front","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
            let config = CompileConfig {
                store: Arc::new(MemStore::default()),
                jobs: hirsel_intern::jobs::Jobs::new(jobs),
                hold_bytes: 0,
                ..Default::default()
            };
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
            let mut scalar = COMPILER
                .with_config(CompileConfig {
                    store: Arc::new(MemStore::default()),
                    ..config.clone()
                })
                .with_owned_engines()
                .load_program(&manifest)
                .unwrap();
            let entry = scalar.entry.clone();
            let expected_revision = scalar.engine.semantic_program_revision(&entry).unwrap();
            drop(scalar);
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
                assert!(checked.diagnostics.is_empty());
            }
            {
                let run = session
                    .request(CompileOperation::Tir { definition: None })
                    .unwrap();
                let CompileResult::Tir(run) = &run.result else {
                    panic!("run result")
                };
                assert_eq!(run.revision, expected_revision);
                use hirsel_tir::TirProvider;
                let mut instances = std::collections::BTreeSet::new();
                for name in ["Point", "Box3"] {
                    let mut path = run.entry.clone();
                    path.0.name = name.into();
                    let actual = run.runtime.types.intern(hirsel_types::Type::with_kind(
                        hirsel_types::TypeKind::Named {
                            definition: hirsel_syntax::DefId::intern(&path),
                            arguments: vec![].into(),
                        },
                    ));
                    for name in ["generated", "inner"] {
                        path.0.name = name.into();
                        let instance = run
                            .provider()
                            .instance_body(&path, &[hirsel_types::GenericArgument::Type(actual)])
                            .unwrap()
                            .expect("the closed generated instance is installed");
                        run.provider().definition(&instance).unwrap();
                        instances.insert(instance);
                    }
                }
                assert_eq!(
                    instances.len(),
                    4,
                    "both outer and inner types are installed"
                );
                for checked in [false, true] {
                    let mut runtime = (*run.runtime).clone();
                    runtime.checked = checked.then(|| (*run.checked_runtime).clone());
                    let execution = hirsel_interp::run_with_runtime(
                        &mut run.provider(),
                        &run.sources,
                        &hirsel_tir::RunConfiguration {
                            entry: run.entry.clone(),
                            hash_seed: 0,
                            inputs: vec![],
                        },
                        vec![],
                        hirsel_interp::Limits {
                            steps: Some(10000),
                            call_depth: 256,
                            source: None,
                        },
                        &mut hirsel_interp::NoOperations,
                        &runtime,
                    )
                    .unwrap();
                    assert!(
                        matches!(
                            &execution.outcome,
                            hirsel_interp::Outcome::Returned {
                                value: hirsel_interp::Value::Integer(value),
                                ..
                            } if value.bits() == 21
                        ),
                        "{checked}: {:?}",
                        execution.outcome
                    );
                }
            }
            let ownership_output = session
                .request(CompileOperation::NativeOwnership(OwnershipRequest {
                    definition: None,
                    target: native::TargetSpec::x86_64_unknown_linux_gnu(),
                    validation: native::Validation::EveryStage,
                    launch: Default::default(),
                    fuel: false,
                }))
                .unwrap();
            let CompileResult::Ownership(ownership) = &ownership_output.result else {
                panic!("ownership result")
            };
            assert!(!ownership.units().is_empty());
            drop(session);
            assert!(budget.reserved() > 0);
            drop(ownership_output);
            assert_eq!(budget.reserved(), 0);
        }
    });
}
