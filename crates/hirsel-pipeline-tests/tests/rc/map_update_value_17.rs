//! Value-only mutation: source/V, H, fault-injected checked heap and both backends.
#[path = "../../../hirsel-driver/tests/support/source_std.rs"]
mod source_std;
use hirsel_driver::native::*;
use hirsel_engine::{
    RunConfiguration, check_snapshot, default_context, entry_path, run_observed_snapshot,
    with_compiler_stack,
};

#[test]
fn value_only_mutation_reference_heap_llvm_cranelift() {
    qualify(&[
        (
            "values",
            include_str!("../../../../conformance/SEM/SEM-5-map-update-value-17-run.hir"),
            Some(0),
            b"",
        ),
        (
            "arena",
            include_str!("../../../../conformance/MEM/MEM-6-map-update-value-17-run.hir"),
            Some(0),
            b"",
        ),
    ]);
}

#[test]
fn value_only_mutation_failure_reference_heap_llvm_cranelift() {
    qualify(&[(
        "failure",
        include_str!("../../../../conformance/OWN/OWN-3-map-update-value-17-failure.hir"),
        None,
        b"changed\n",
    )]);
}

#[test]
fn value_only_qualified_receipts_reference_heap_llvm_cranelift() {
    qualify(&[(
        "qualified",
        r#"pub fn main() -> Result[(), String] {
  let mut m: Map[u64, unique List[u64]] = map.empty()
  map.insert(mut m, 1, [10])
  let saved = copy m
  let _ = std@1/map.update_value(mut m, 1, fn(mut v: unique List[u64]) -> () uses {} { list.push(mut v, 20) })
  failure.check(map.get(saved, 1) == Some([10u64]) && map.get(m, 1) == Some([10u64, 20u64]), Bounds())
  Ok(())
}
"#,
        Some(0),
        b"",
    )]);
}

fn qualify(cases: &[(&str, &str, Option<i32>, &[u8])]) {
    with_compiler_stack(|| {
        let root =
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("map-update-value-17");
        let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        options.validation = Validation::EveryStage;
        let configurations = [
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::Speed,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
        ];
        let tools = Toolchain::for_matrix(&root.join("runtime"), &configurations).unwrap();
        for &(name, source, exit, output) in cases {
            let mut engine = source_std::engine();
            let file = engine
                .sources
                .add(format!("{name}.hir"), source.into())
                .unwrap();
            let diagnostics = check_snapshot(&mut engine, file, &default_context()).unwrap();
            assert!(diagnostics.is_empty(), "{name}: {diagnostics:?}");
            let entry = entry_path(&default_context());
            let execution = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                Default::default(),
            )
            .unwrap();
            let reference = Observation::of_reference(execution.observation.as_ref().unwrap());
            assert_eq!(reference.output, output, "{name}");
            assert_eq!(reference.exit_code, exit, "{name}: {reference:?}");
            let input = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(input).unwrap();
            let ready = build(&validated, &options).unwrap();
            let sites = Sites::new(validated.snapshot());
            // Audit every failure-capable site as well as the clean execution.
            let heap = heap_campaign(&ready, &entry, &sites).unwrap();
            assert_eq!(heap.observation, reference, "{name}");
            assert_eq!(heap.counters.live_heap_bytes, 0, "{name}");
            eprintln!(
                "{name}: audited {} failure sites; {:?}",
                heap.sites, heap.counters
            );
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let outcomes = differential(
                &validated,
                &entry,
                &reference,
                &configurations,
                &tools,
                &dir,
            );
            assert_eq!(outcomes.len(), 2);
            for (configuration, outcome) in outcomes {
                assert_eq!(outcome, Outcome::Agree, "{name}: {configuration}");
            }
        }
    });
}
