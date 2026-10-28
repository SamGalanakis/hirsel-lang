use hirsel_driver::native::{Evidence, Proofs};
use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::ResultStore, *};
use std::sync::Arc;

#[test]
fn complete_native_export_assessment_drives_actual_discharge_after_source_host_drop() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(
            directory.path().join("main.hir"),
            r#"
fn bump(xs:List[u32],i:u64,y:u32)->u32 ensures result>=y {xs[i]+y}
pub fn main()->Result[(),()] {let n=bump([1u32,2u32],1u64,3u32);Ok(())}
"#,
        )
        .unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"proof_native","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let store: Arc<dyn ResultStore> =
            Arc::new(store::DirStore::new(directory.path().join("products")));
        let jobs = 1;
        let mut program = COMPILER
            .with_config(CompileConfig {
                store: store.clone(),
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
            retained: 2 << 20,
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
                budget.reserve(8 << 20).unwrap(),
                errors,
            )
            .unwrap();
        let lower = program
            .engine
            .front_lower_queries(
                &definitions,
                front.clone(),
                weight,
                32,
                budget.reserve(8 << 20).unwrap(),
                errors,
            )
            .unwrap();
        let growth = program
            .engine
            .front_growth_queries(
                &program.entry,
                front.clone(),
                weight,
                budget.reserve(8 << 20).unwrap(),
                errors,
            )
            .unwrap();
        let check = program
            .engine
            .front_check_queries(
                FrontCheckPlan {
                    files: program
                        .files
                        .iter()
                        .map(|(file, context)| (*file, context.clone(), LoweringPolicy::Comptime))
                        .collect(),
                    front: front.clone(),
                    lower: lower.clone(),
                    growth: growth.clone(),
                    unsafe_ffi: program.unsafe_ffi.clone(),
                    lint_package: None,
                },
                weight,
                budget.reserve(8 << 20).unwrap(),
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
            .register(FRONT_GROWTH_KIND, growth.factory())
            .unwrap();
        runtime.register(FRONT_CHECK_KIND, check.factory()).unwrap();
        let headers = program
            .engine
            .front_checked_signatures(front.clone(), budget.reserve(4 << 20).unwrap())
            .unwrap();
        let run = program
            .engine
            .front_run_queries(
                check.clone(),
                front.clone(),
                lower.clone(),
                weight,
                budget.reserve(16 << 20).unwrap(),
                errors,
            )
            .unwrap();

        let catalogue = program
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
        let source = program
            .engine
            .front_native_queries(
                FrontNativePlan {
                    entry: program.entry.clone(),
                    roots: Vec::new(),
                    run,
                    definitions: catalogue,
                },
                weight,
                budget.reserve(16 << 20).unwrap(),
                errors,
            )
            .unwrap();
        let export = program
            .engine
            .front_proof_export_from_native(
                &program.entry,
                &source,
                headers,
                budget.reserve(4 << 20).unwrap(),
            )
            .unwrap();
        runtime
            .register(FRONT_RUN_KIND, source.run_factory())
            .unwrap();
        runtime
            .register(FRONT_NATIVE_DEFINITION_KIND, source.definition_factory())
            .unwrap();
        runtime
            .register(FRONT_NATIVE_ROOT_KIND, source.factory())
            .unwrap();
        let request = runtime.request(&[source.query()]);
        let completed = runtime
            .wait(request)
            .unwrap_or_else(|r| panic!("{:?}", source.failure(r)))[0];
        let input = export.input_for_native(&source, completed).unwrap();
        let table = hirsel_proof_export::build(&input);
        let checker = hirsel_proof_export::sidecar::Checker::parse("prover@1").unwrap();
        let records = table
            .definitions
            .iter()
            .flat_map(|d| &d.obligations)
            .filter(|o| {
                o.id.to_string().contains("/bump#FACT-1/")
                    || o.id.to_string().contains("/bump#SEM-1/")
            })
            .map(|o| hirsel_proof_export::sidecar::Record {
                revision: table.header.program_revision.unwrap(),
                obligation: o.id.to_string(),
                checksum: o.checksum,
                checker: checker.clone(),
                budget: hirsel_proof_export::sidecar::Budget {
                    unit: "steps".into(),
                    count: 1000,
                },
                verdict: hirsel_proof_export::sidecar::Verdict::Proved,
            })
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        let evidence = Evidence {
            records,
            trusted: vec![checker],
        };
        let actual = source.outcome(completed).unwrap().products;
        let declarations = native::queries::declarations::NativeDeclarations::from_settled(
            &actual.signatures,
            actual.roots.clone(),
            actual.catalogue.iter().map(|p| (**p).clone()),
            actual.statics.clone(),
            actual.extra.clone(),
        )
        .unwrap();
        let original = actual
            .original_context
            .reader(
                actual.originals.iter().map(|p| (**p).clone()),
                store.clone(),
            )
            .unwrap();
        let inputs = native::queries::source_inputs::NativeInstanceInputs::from_products(
            declarations,
            original,
        )
        .unwrap();
        runtime.shutdown();
        drop(runtime);
        drop(program);
        let trace = hirsel_lir::construction::ValueTrace::default();
        let _observe = trace.enter();
        let proofs =
            Proofs::assess_native(&export, &source, completed, &evidence, store.clone()).unwrap();
        assert!(!proofs.discharge.is_empty());
        assert_eq!(trace.totals(), Default::default());
        let wrong = ProductRef {
            action: store::Action(Default::default()),
            ..completed
        };
        assert!(Proofs::assess_native(&export, &source, wrong, &evidence, store.clone()).is_err());
        let mut untrusted = evidence.clone();
        untrusted.trusted.clear();
        assert!(
            Proofs::assess_native(&export, &source, completed, &untrusted, store.clone())
                .unwrap()
                .discharge
                .is_empty()
        );
        let mut stale = evidence.clone();
        for record in &mut stale.records {
            record.revision = Default::default();
        }
        assert!(
            Proofs::assess_native(&export, &source, completed, &stale, store.clone())
                .unwrap()
                .discharge
                .is_empty()
        );
        let mut duplicate = evidence.clone();
        duplicate.records.push(duplicate.records[0].clone());
        assert!(
            Proofs::assess_native(&export, &source, completed, &duplicate, store.clone()).is_err()
        );
        let mut options = native::Options::new(hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu());
        options.validation = native::Validation::Ready;
        options.discharge = proofs.discharge.clone();
        let tasks = native::queries::instance_tasks::InstanceTasks::new(
            inputs,
            store.clone(),
            options,
            hirsel_syntax::ContentHash::of_bytes(b"complete native proofs"),
            weight,
            budget.reserve(64 << 20).unwrap(),
        )
        .unwrap();
        let mut runtime = AdmittedRuntime::new(
            hirsel_intern::jobs::Jobs::new(1),
            budget.reserve(512 << 20).unwrap(),
            errors,
        )
        .unwrap();
        let factory = Arc::new(native::queries::instance_tasks::InstanceTaskFactory::new(
            tasks.clone(),
        ));
        for kind in [
            native::queries::INSTANCE_KIND,
            native::queries::instance_tasks::INSTANCE_INPUT_KIND,
            native::queries::instance_tasks::DISCOVERY_KIND,
            native::queries::instance_tasks::CALLABLE_GRAPH_KIND,
            native::queries::instance::CALLABLE_VALUE_KIND,
        ] {
            runtime.register(kind, factory.clone()).unwrap();
        }
        let request = runtime.request(&[tasks.discovery_id()]);
        let settled = runtime.wait(request).unwrap_or_else(|r| {
            panic!(
                "{}",
                String::from_utf8_lossy(&store.get_content(r).unwrap())
            )
        })[0];
        let removed = tasks.discharged(settled).unwrap();
        assert_eq!(removed, proofs.discharge);
        let report = proofs.report_discharged(&removed);
        assert_eq!(report.iter().filter(|s| s.contains(": removed")).count(), 2);
        assert!(
            report
                .iter()
                .any(|s| s.starts_with("proofs: 2 removed, 0 kept,"))
        );
    });
}
