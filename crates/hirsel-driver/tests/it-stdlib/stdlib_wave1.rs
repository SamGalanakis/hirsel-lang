use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_lir::ir::UnitOrigin;
use std::{collections::BTreeSet, path::PathBuf};

fn source(prefix: &str, name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("conformance")
        .join(prefix)
        .join(format!("{name}.hir"))
}

#[test]
fn callback_failure_releases_current_remaining_and_retained_resources_in_order() {
    with_compiler_stack(|| {
        for (name, expected, failure) in [
            (
                "DROP-2-wave1-callback-failure",
                b"B\nC\nA\n".as_slice(),
                true,
            ),
            (
                "DROP-2-wave1-callback-success",
                b"C\nB\nA\n".as_slice(),
                false,
            ),
            ("DROP-2-wave1-fold-resource-fail", b"A\n".as_slice(), true),
            ("DROP-2-wave1-fold-resource-run", b"A\n".as_slice(), false),
        ] {
            let mut output = Vec::new();
            let outcome = run_path(&source("DROP", name), &mut output).unwrap();
            assert_eq!(output, expected, "{name}");
            if failure {
                assert!(
                    matches!(outcome, RunOutcome::Failed { primary: hirsel_run_schema::Failure { ref rule, .. }, .. } if rule.as_str() == "MEM-14")
                );
            } else {
                assert!(matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ));
            }
        }
    });
}

/// Builds one program natively and checks that it reaches each of `functions`
/// (`(module, name)`) as an ordinary std source unit. Each program is its own
/// case, so the cases run in parallel shards.
fn std_algorithms_are_ordinary_source_native_units(name: &str, functions: &[(&str, &str)]) {
    with_compiler_stack(|| {
        let mut found = BTreeSet::new();
        let mut loaded = load_program(&source("SEM", name)).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        for unit in &ready.units {
            let header = &unit.unit().header;
            if header.definition.0.package.name_is("std") {
                assert_eq!(header.origin, UnitOrigin::Source);
                found.insert((
                    header.definition.0.modules.join("/"),
                    header.definition.0.name.clone(),
                ));
            }
        }
        for (module, function) in functions {
            assert!(
                found.contains(&(module.to_string(), function.to_string())),
                "{name}: missing {module}/{function}: {found:?}"
            );
        }
    });
}

#[test]
fn every_wave1_list_algorithm_is_an_ordinary_source_native_unit() {
    std_algorithms_are_ordinary_source_native_units(
        "SEM-4-wave1-counted-run",
        &[
            "sort",
            "reverse",
            "rotate",
            "binary_search",
            "find",
            "map",
            "filter",
            "fold",
            "all",
            "any",
            "partition",
            "concat",
            "dedup",
        ]
        .map(|name| ("list", name)),
    );
}

#[test]
fn wave1_text_building_is_an_ordinary_source_native_unit() {
    std_algorithms_are_ordinary_source_native_units(
        "SEM-3-wave1-byte-alias-run",
        &[("text", "append"), ("text", "finish")],
    );
}

#[test]
fn wave1_string_split_is_an_ordinary_source_native_unit() {
    std_algorithms_are_ordinary_source_native_units(
        "SEM-3-wave1-split-delimiter-run",
        &[("string", "split")],
    );
}

/// A package program's native input selects its own package's definitions,
/// reachable or not, and only the dependency definitions those reach: std's
/// exact summation and its superaccumulator, none of the rest of std.
#[test]
fn a_package_program_exports_only_the_dependency_definitions_it_reaches() {
    with_compiler_stack(|| {
        let dir = std::env::temp_dir().join(format!(
            "hirsel-native-selection-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("selection.hir");
        std::fs::write(
            &program,
            "fn unused(x: u64) -> u64 { x + 1 }\n\
             pub fn main(t: Test) -> Result[(), ()] {\n\
             \x20 let xs: Array[f32] = [1.0f32, 2.0f32]\n\
             \x20 core@1/test.check(t, std@1/f32.sum_exact(xs) == 3.0f32)\n\
             \x20 Ok(())\n\
             }\n",
        )
        .unwrap();
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        std::fs::write(
            dir.join("selection.hir.json"),
            serde_json::json!({
                "schema": 1,
                "package": {"name": "application", "version": "1"},
                "entry": {"module": [], "name": "main"},
                "sources": [{"path": "selection.hir", "module": []}],
                "dependencies": [{
                    "manifest": manifest,
                    "package": {"name": "std", "version": "1"},
                    "expose": []
                }]
            })
            .to_string(),
        )
        .unwrap();
        let mut loaded = load_program(&program).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let selected: BTreeSet<_> = input
            .definitions
            .keys()
            .map(|path| {
                let item = &path.0;
                (
                    item.package.name.clone(),
                    item.modules.join("/"),
                    item.name.clone(),
                )
            })
            .collect();
        let expected: BTreeSet<_> = [
            ("application", "", "main"),
            ("application", "", "unused"),
            ("std", "f32", "sum_exact"),
            ("std", "internal/superacc", "acc_add"),
            ("std", "internal/superacc", "acc_add_word"),
            ("std", "internal/superacc", "acc_finish"),
            ("std", "internal/superacc", "acc_zero"),
        ]
        .into_iter()
        .map(|(p, m, n)| (p.to_string(), m.to_string(), n.to_string()))
        .collect();
        assert_eq!(selected, expected);
        // The pruned selection imports and builds.
        let validated = loaded.engine.import_native_input(input).unwrap();
        native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    });
}
