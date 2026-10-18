//! Source-origin checks cannot be expressed by a conformance program.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_lir::ir::UnitOrigin;
use std::path::PathBuf;

fn source_codec(name: &str, module: &str, functions: &[&str]) {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../conformance/SEM/SEM-3-{name}-run.hir"));
        let mut loaded = load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| match error {
                ToolError::Query { error, .. } => panic!("{error:?}"),
                error => panic!("{error}"),
            });
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        for function in functions {
            let unit = ready
                .units
                .iter()
                .find(|unit| {
                    let def = &unit.unit().header.definition.0;
                    def.package.name_is("std") && def.modules == [module] && def.name == *function
                })
                .unwrap_or_else(|| panic!("missing {module}/{function}"));
            assert_eq!(unit.unit().header.origin, UnitOrigin::Source);
        }
        assert!(matches!(
            run_path(&path, &mut Vec::new()).unwrap(),
            RunOutcome::Exit(0)
        ));
    });
}

#[test]
fn hex_is_ordinary_source() {
    source_codec("hex", "hex", &["encode", "decode"]);
}

#[test]
fn base64_is_ordinary_source() {
    source_codec("base64", "base64", &["encode", "decode"]);
}

#[test]
fn utf8_is_ordinary_source() {
    source_codec("utf8", "utf8", &["append_scalar", "decode_lossy"]);
}

#[test]
fn common_strings_are_ordinary_source() {
    source_codec("string-common", "string", &["contains", "lines", "replace"]);
}
