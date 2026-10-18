use hirsel_driver::*;
use hirsel_engine::with_compiler_stack;
use hirsel_lir::ir::UnitOrigin;
use std::{collections::BTreeSet, path::PathBuf};

// Corpus results cannot detect a hidden primitive replacing a source kernel.
#[test]
fn float_math_is_ordinary_source_native_code() {
    with_compiler_stack(|| {
        let mut found = BTreeSet::new();
        for name in [
            "SEM-2-std-float-f32-math-run.hir",
            "SEM-2-std-float-f64-math-run.hir",
        ] {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../conformance/SEM")
                .join(name);
            let mut loaded = load_program(&path).unwrap();
            loaded
                .check_for_execution()
                .unwrap_or_else(|error| panic!("{error}"));
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
                    if header.definition.0.modules.len() == 1 {
                        found.insert((
                            header.definition.0.modules[0].to_string(),
                            header.definition.0.name.to_string(),
                        ));
                    }
                }
            }
        }
        for ty in ["f32", "f64"] {
            for name in ["floor", "ceil", "round", "sqrt"] {
                assert!(
                    found.contains(&(ty.into(), name.into())),
                    "missing source {ty}.{name}"
                );
            }
        }
    });
}
