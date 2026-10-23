//! GEN-6 package aliases select their terminal callable bodies and actuals.
use crate::rc_support::history::{Category, Runner};
use hirsel_driver::{load_program, native::*};
use hirsel_engine::*;
use std::path::Path;

fn compare(name: &str, failure: bool) {
    with_compiler_stack(|| {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/GEN/dictionary-identity-22")
            .join(format!("{name}.json"));
        let mut program = load_program(&fixture).expect("manifest setup");
        program.check_for_execution().expect("source acceptance");
        let input = program.engine.export_native_input().expect("V export");
        let validated = program.engine.import_native_input(input).expect("V import");
        let run = run_observed_snapshot(
            &mut program.engine,
            &RunConfiguration {
                entry: program.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .expect("reference execution");
        let reference = Observation::of_reference(&run.observation.expect("reference observation"));
        let directory = tempfile::tempdir().unwrap();
        let matrix = [
            Engine::Heap { campaign: true },
            Engine::Native {
                opt: OptLevel::None,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
            Engine::Native {
                opt: OptLevel::Default,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
            Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        ]
        .into_iter()
        .map(|engine| Configuration {
            options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
            engine,
        })
        .collect();
        let runner = Runner::with_matrix(directory.path(), matrix).unwrap();
        let report = runner.compare(&validated, &program.entry, &reference, name);
        eprintln!("DICTIONARY_IDENTITY_22_ENGINES {name}: {report:#?}");
        assert!(
            report.validated && report.cells.len() == 5 && report.agrees(),
            "{report:#?}"
        );
        for cell in &report.cells {
            assert_eq!(cell.category, Category::Observation);
            let o = cell.observation.as_ref().unwrap();
            assert!(
                o.output.is_empty()
                    && o.roots.is_empty()
                    && !o.blocked
                    && o.budget.is_none()
                    && o.signaled.is_none(),
                "{o:?}"
            );
            // Every exact callback return is checked before Ok. The failure
            // twin divides by right(1,0): selecting left would return.
            assert_eq!(o.exit_code, if failure { None } else { Some(0) }, "{o:?}");
            assert_eq!(o.failures.len(), usize::from(failure), "{o:?}");
            if failure {
                let f = &o.failures[0];
                assert_eq!(f.rule, "SEM-1");
                assert_eq!(f.definition, program.entry);
                assert_eq!(f.line, 10);
                assert!(f.operation.is_none(), "{f:?}");
            }
        }
    });
}

#[test]
fn callable_returns_across_packages() {
    compare("return", false);
}

#[test]
fn distinct_callback_failure_across_packages() {
    compare("failure", true);
}
