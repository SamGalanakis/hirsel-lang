use hirsel_driver::*;
use hirsel_engine::*;
fn path(name: &str) -> DefPath {
    let mut p = entry_path(&default_context());
    p.0.name = name.into();
    p
}
fn engine(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("comptime.hir", source.into()).unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    engine
}

fn placed_evidence(sources: &hirsel_span::SourceMap, text: String) -> String {
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find("Anchor(") {
        out.push_str(&rest[..at]);
        rest = &rest[at + "Anchor(".len()..];
        let end = rest.find(')').unwrap();
        let anchor = hirsel_span::Anchor(rest[..end].parse().unwrap());
        let place = sources
            .anchor(anchor)
            .map(|(file, base)| (sources.file(file).unwrap().path.clone(), base));
        out.push_str(&format!("Anchor({place:?})"));
        rest = &rest[end + 1..];
    }
    out + rest
}
fn outcome_text(
    engine: &QueryEngine,
    result: Result<hirsel_tir::Materialized, QueryError>,
) -> Result<String, String> {
    result
        .map(|value| materialized_text(engine, &value))
        .map_err(|error| placed_evidence(&engine.sources, format!("{error:?}")))
}
fn materialized_text(engine: &QueryEngine, value: &hirsel_tir::Materialized) -> String {
    let context = hirsel_proof_export::Cx::new(
        std::rc::Rc::new(|_| hirsel_proof_export::Location::unknown()),
        engine.types().clone(),
    );
    hirsel_codec::to_text_with(value, context).0
}
#[test]
fn a_failed_size_instantiation_retains_its_selecting_site() {
    use hirsel_driver::requests::{CompileOperation, CompileResult, CompilerRequests};
    with_compiler_stack(|| {
        let source = include_str!("../../../../conformance/CT/CT-1-size-instantiation-bad.hir");
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("comptime.hir"), source).unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"standalone","version":"0"},"sources":[{"path":"comptime.hir","module":[]}],"dependencies":[]}"#).unwrap();
        for jobs in [1, 2, 8] {
            let config = CompileConfig {
                jobs: hirsel_intern::jobs::Jobs::new(jobs),
                store: std::sync::Arc::new(store::MemStore::default()),
                hold_bytes: 0,
                ..Default::default()
            };
            for warm in [false, true] {
                let mut requests = CompilerRequests::for_host(
                    COMPILER.with_config(config.clone()),
                    config.clone(),
                );
                let output = requests
                    .execute(
                        &manifest,
                        CompileOperation::Check {
                            lowering: LoweringPolicy::Comptime,
                        },
                    )
                    .unwrap_or_else(|error| panic!("jobs={jobs} warm={warm}: {error}"));
                let CompileResult::Check(checked) = &output.result else {
                    panic!("expected checked source");
                };
                let diagnostic = checked
                    .diagnostics
                    .iter()
                    .find(|d| d.rule.as_str() == "CT-1")
                    .unwrap();
                assert!(
                    checked
                        .sources
                        .slice(diagnostic.primary)
                        .unwrap()
                        .starts_with("comptime")
                );
                assert!(
                    diagnostic
                        .secondary
                        .iter()
                        .any(
                            |label| checked.sources.slice(label.span) == Some("inverse(second)")
                                && label.message.contains("[0]")
                        ),
                    "{diagnostic:?}"
                );
            }
        }
    });
}

#[test]
fn frozen_frontier_staged_rounds_evaluate_on_workers() {
    use std::sync::Arc;
    struct WorkerOnly(Arc<dyn LoweringPhase>, bool);
    impl LoweringPhase for WorkerOnly {
        fn lower(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            body: &AnalyzedBody,
            comptime: &Comptime,
            signature: &hirsel_types::Signature,
        ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
            self.0.lower(t, m, body, comptime, signature)
        }
        fn lower_evaluation(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            body: &AnalyzedBody,
            staged: &[(hirsel_syntax::ExprId, hirsel_types::TypeId)],
            signature: &hirsel_types::Signature,
            expression: hirsel_syntax::ExprId,
            result: hirsel_types::TypeId,
            constants: &[(hirsel_syntax::ExprId, hirsel_tir::Materialized)],
        ) -> Result<hirsel_tir::Definition, QueryError> {
            if self.1 {
                assert_eq!(
                    std::thread::current().name(),
                    Some("hirsel-query"),
                    "staged demand fell back to caller evaluation"
                );
            }
            self.0
                .lower_evaluation(t, m, body, staged, signature, expression, result, constants)
        }
        fn entry_body(&self) -> hirsel_tir::BodyId {
            self.0.entry_body()
        }
    }
    thread_local! {
        static REQUIRE_WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    fn worker_adapters() -> PhaseAdapters {
        let mut adapters = compiler_adapters();
        adapters.lowering = Some(Arc::new(WorkerOnly(
            adapters.lowering.take().unwrap(),
            REQUIRE_WORKER.with(|required| required.get()),
        )));
        adapters
    }
    with_compiler_stack(|| {
        let staged = "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime n*n}\nconst A:u64=comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)}\nconst B:u64=comptime {let xs:Inline[u8,4]=[1,2,3,4];scaled(xs)}\n";
        let generated = "type Seed {x:u32}\nfn body_text()->String{\"fn()->u32 uses {}{11u32}\"}\nfn make(s:TypeShape[Seed])->Result[Body[fn()->u32 uses {}],String]{Ok(codegen.body(body_text()))}\nfn chosen()->u32 uses {}=comptime make(shape(Seed))\nconst A:u32=comptime chosen()\nconst B:u32=comptime chosen()\n";
        let generic = "type Point {x:u64}\nfn make[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {}],String]{Ok(codegen.body(\"fn(p:standalone@0/Point)->u64 uses {}{12u64}\"))}\nfn width[T](p:T)->u64 uses {}=comptime make(shape(T))\nconst A:u64=comptime width(Point(x:1))\nconst B:u64=comptime width(Point(x:2))\n";
        for source in [staged, generated, generic] {
            for jobs in [1, 2, 8] {
                let paths = vec![path("B"), path("A"), path("B")];
                let mut control = engine(source);
                let expected: Vec<_> = paths
                    .iter()
                    .map(|p| {
                        let result = control.materialize_value(p);
                        outcome_text(&control, result)
                    })
                    .collect();
                assert!(
                    expected.iter().all(Result::is_ok),
                    "{expected:?}; jobs={jobs}; source={source}; phase={:?}",
                    control.last_phase
                );
                use hirsel_driver::requests::{CompileOperation, CompileResult, CompilerRequests};
                let directory = tempfile::tempdir().unwrap();
                let manifest = directory.path().join("hirsel.json");
                std::fs::write(directory.path().join("comptime.hir"), source).unwrap();
                std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"standalone","version":"0"},"sources":[{"path":"comptime.hir","module":[]}],"dependencies":[]}"#).unwrap();
                let config = CompileConfig {
                    jobs: hirsel_intern::jobs::Jobs::new(jobs),
                    store: Arc::new(store::DirStore::new(directory.path().join("products"))),
                    hold_bytes: 0,
                    ..Default::default()
                };
                REQUIRE_WORKER.with(|required| required.set(jobs > 1));
                let compiler = COMPILER.with_config(config.clone()).with_phase_adapters(
                    worker_adapters,
                    hirsel_syntax::ContentHash::of_bytes(b"hirsel.staged-request-worker.v1").0,
                );
                let mut candidate = CompilerRequests::for_host(compiler, config);
                for warm in [false, true] {
                    let output = candidate
                        .execute(&manifest, CompileOperation::Precompute)
                        .unwrap_or_else(|error| panic!("jobs={jobs} warm={warm}: {error}"));
                    let CompileResult::Precomputed { sources, .. } = &output.result else {
                        panic!("expected source precompute");
                    };
                    let actual: Vec<Result<String, String>> = paths
                        .iter()
                        .map(|path| {
                            let bound = sources.source_product_bytes(path).unwrap().unwrap();
                            let budget = hirsel_engine::runtime::ProcessBudget::new(bound);
                            let products = sources
                                .source_products(path, budget.reserve(bound).unwrap())
                                .unwrap()
                                .unwrap();
                            let context = hirsel_proof_export::Cx::new(
                                std::rc::Rc::new(|_| hirsel_proof_export::Location::unknown()),
                                sources.signatures().types().clone(),
                            );
                            Ok(hirsel_codec::to_text_with(products.constant().unwrap(), context).0)
                        })
                        .collect();
                    assert_eq!(actual, expected, "jobs={jobs} warm={warm}");
                    if source == generated {
                        let scalar = control.check_definition(&path("chosen")).unwrap();
                        let bound = sources
                            .checked_dependency_decode_bytes(&path("chosen"))
                            .unwrap();
                        let budget = hirsel_engine::runtime::ProcessBudget::new(bound);
                        let worker = sources
                            .checked_dependencies(&path("chosen"), budget.reserve(bound).unwrap())
                            .unwrap();
                        assert!(
                            scalar
                                .dependencies
                                .iter()
                                .any(|d| *d.definition.path() == path("make"))
                        );
                        assert_eq!(
                            worker.dependencies(),
                            &*scalar.dependencies,
                            "checked dependencies jobs={jobs} warm={warm}"
                        );
                    }
                }
            }
        }
    });
}

#[test]
fn runtime_calls_select_staged_sizes_from_settled_summaries_after_restore() {
    use hirsel_driver::requests::{CompileOperation, CompileResult, CompilerRequests};
    use hirsel_tir::TirProvider;
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("main.hir"),
            "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime (n*n)}\npub fn main()->Result[(),()]{let a:Inline[u8,2]=[1,2];let b:Inline[u8,3]=[1,2,3];let _=scaled(a)+scaled(b);Ok(())}\n").unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"runtime_sizes","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        for jobs in [1, 2, 8] {
            let store = Arc::new(MemStore::default());
            let config = CompileConfig {
                store,
                jobs: hirsel_intern::jobs::Jobs::new(jobs),
                hold_bytes: 0,
                ..Default::default()
            };
            for warm in [false, true] {
                let mut requests = CompilerRequests::for_host(
                    COMPILER.with_config(config.clone()),
                    config.clone(),
                );
                let output = requests
                    .execute(&manifest, CompileOperation::Tir { definition: None })
                    .unwrap_or_else(|error| panic!("jobs={jobs} warm={warm}: {error}"));
                let CompileResult::Tir(input) = &output.result else {
                    panic!("expected immutable TIR");
                };
                let mut provider = input.provider();
                let mut scaled = input.entry.clone();
                scaled.0.name = "scaled".into();
                let definition = provider.definition(&scaled).unwrap();
                let hidden = definition
                    .bodies
                    .iter()
                    .flat_map(|b| &b.rvalues)
                    .find_map(|value| match value {
                        hirsel_tir::Rvalue::ReadConstant {
                            definition,
                            arguments,
                        } if !arguments.is_empty() => Some(definition.clone()),
                        _ => None,
                    })
                    .expect("generic staged read");
                for n in [2, 3] {
                    assert_eq!(
                        provider.constant_instance(&hidden, &[n]).unwrap().root.kind,
                        hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(u128::from(
                            n * n
                        )))
                    );
                }
                assert!(
                    provider.constant_instance(&hidden, &[4]).is_err(),
                    "unselected size stays absent"
                );
            }
        }
    });
}
