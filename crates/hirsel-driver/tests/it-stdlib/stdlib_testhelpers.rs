use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::observation::Effect;
use hirsel_lir::ir::UnitOrigin;
use std::{collections::BTreeSet, path::PathBuf};

fn details(error: &ToolError) -> String {
    match error {
        ToolError::Query { error, .. } => format!("{error:?}"),
        _ => error.to_string(),
    }
}

fn source(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/LANG")
        .join(format!("LANG-9-testhelpers-{name}.hir"))
}

#[test]
fn landed_test_helpers_are_source_units_and_resource_cases_finalize_once() {
    with_compiler_stack(|| {
        let text = r#"
resource type Watch { n: u8 }
@noalloc drop(w) { core@1/failure.check(w.n == 7, Arithmetic()) }
fn generate(mut s: test.Source) -> Watch { Watch(n: core@1/int.narrow(test.range_u64(mut s, 7, 7))) }
fn law(value: Watch) -> bool uses {} { value.n == 7 }
fn show(value: Watch) -> String { u8.to_string(value.n) }
pub fn trial(t: Test) -> Result[(), ()] {
  let counted: String = "counted"
  std@1/test.equal(t, counted, counted)
  std@1/test.approx(t, 12.06, 12.0, 0.1, 0.0)
  std@1/test.property(t, generate, law, show)
  Ok(())
}
"#;
        let mut loaded = load_program_source(&source("property-run"), Some(text))
            .unwrap_or_else(|e| panic!("{}", details(&e)));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{}", details(&e)));
        let reference = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert!(reference.outcome.is_ok(), "{:?}", reference.outcome);
        let observation = reference.observation.as_ref().unwrap();
        assert_eq!(observation.effects.iter().filter(|effect| matches!(&effect.effect, Effect::Finalizer { definition } if definition.0.name == "Watch")).count(), 100);
        let expected = native::Observation::of_reference(observation);
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let found: BTreeSet<_> = ready
            .units
            .iter()
            .filter_map(|unit| {
                let header = &unit.unit().header;
                if header.definition.0.package.name_is("std")
                    && header.definition.0.modules == ["test"]
                {
                    assert_eq!(header.origin, UnitOrigin::Source);
                    Some(header.definition.0.name.as_str())
                } else {
                    None
                }
            })
            .collect();
        for name in [
            "equal",
            "approx",
            "property",
            "property_cases",
            "range_u64",
            "choice",
        ] {
            assert!(found.contains(name), "missing {name}: {found:?}");
            assert!(
                registry::BuiltinRegistry::new()
                    .get(&hirsel_syntax::DefPath::core(&format!("test.{name}")))
                    .is_none()
            );
        }
        let actual = native::heap_campaign(
            &ready,
            &loaded.entry,
            &native::Sites::new(validated.snapshot()),
        )
        .unwrap();
        assert_eq!(actual.observation, expected);
    });
}

/// Each test seed is its own case, so the native builds run in parallel shards.
fn property_reports_use_the_recorded_test_seed_and_first_failing_case(seed: u64) {
    with_compiler_stack(|| {
        let mut loaded = load_program(&source("property-case-fail"))
            .unwrap_or_else(|e| panic!("{}", details(&e)));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{}", details(&e)));
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let reference = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 91,
                inputs: vec![],
            },
            SimulationConfig {
                test_seed: seed,
                random_seed: 17,
                ..Default::default()
            },
        )
        .unwrap();
        let expected = native::Observation::of_reference(reference.observation.as_ref().unwrap());
        assert_eq!(
            expected.output,
            format!("test log: counterexample: 0\ntest failure: seed={seed} case=0\n").as_bytes()
        );
        assert_eq!(expected.failures.len(), 1);
        assert_eq!(expected.failures[0].rule, "LANG-9");
        assert_eq!(
            expected.failures[0].definition.0.package.name,
            "application"
        );
        let operation = expected.failures[0].operation.as_ref().unwrap();
        assert_eq!(operation.definition.0.package.name, "std");
        assert_eq!(operation.definition.0.name, "property_cases");
        let mut options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        options.launch.test_seed = seed;
        options.launch.random_seed = 17;
        options.launch.hash_seed = 91;
        let ready = native::build(&validated, &options).unwrap();
        let actual = native::run_heap(
            &ready,
            &loaded.entry,
            &native::Sites::new(validated.snapshot()),
        )
        .unwrap();
        assert_eq!(actual.observation, expected);
    });
}

#[test]
fn property_reports_use_test_seed_zero_and_the_first_failing_case() {
    property_reports_use_the_recorded_test_seed_and_first_failing_case(0);
}

#[test]
fn property_reports_use_test_seed_five_and_the_first_failing_case() {
    property_reports_use_the_recorded_test_seed_and_first_failing_case(5);
}

#[test]
fn property_reports_use_the_largest_test_seed_and_the_first_failing_case() {
    property_reports_use_the_recorded_test_seed_and_first_failing_case(u64::MAX);
}

/// Each program is its own case, so the native builds run in parallel shards.
fn failed_resource_laws_release_the_current_case_before_reporting(name: &str) {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/DROP")
            .join(format!("DROP-2-testhelpers-{name}.hir"));
        let mut loaded = load_program(&path).unwrap_or_else(|e| panic!("{}", details(&e)));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{}", details(&e)));
        let reference = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let observation = reference.observation.as_ref().unwrap();
        let finalizers: Vec<_> = observation.effects.iter().enumerate().filter_map(|(index, effect)| matches!(&effect.effect, Effect::Finalizer { definition } if definition.0.name == "Watch").then_some(index)).collect();
        if name == "property-resource-fail" {
            // The failing case, each shrinking replay and the counterexample's
            // rendering replay each release their value before the report.
            let report = observation.effects.iter().position(|effect| matches!(&effect.effect, Effect::CapabilityConsumed(event) if event.operation == "test.check_case")).unwrap();
            assert!(finalizers.len() >= 2, "{name}");
            assert!(finalizers.iter().all(|&finalizer| finalizer < report));
        } else {
            assert_eq!(finalizers.len(), 1, "{name}");
        }
        assert_eq!(observation.failures.len(), 1);
        let expected = native::Observation::of_reference(observation);
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let actual = native::heap_campaign(
            &ready,
            &loaded.entry,
            &native::Sites::new(validated.snapshot()),
        )
        .unwrap();
        assert_eq!(actual.observation, expected, "{name}");
    });
}

#[test]
fn a_failed_property_resource_case_is_released_before_its_report() {
    failed_resource_laws_release_the_current_case_before_reporting("property-resource-fail");
}

#[test]
fn a_failed_resource_law_releases_its_case_once() {
    failed_resource_laws_release_the_current_case_before_reporting("property-law-resource-fail");
}
