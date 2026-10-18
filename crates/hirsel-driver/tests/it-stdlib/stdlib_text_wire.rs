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
        let outcome = run_path(&path, &mut Vec::new()).unwrap();
        assert!(matches!(outcome, RunOutcome::Exit(0)), "{outcome:?}");
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

#[test]
fn sha256_is_ordinary_source() {
    source_codec("sha256", "sha256", &["new", "update", "finish", "digest"]);
}

// The million-byte vector runs only as a built executable; bounded corpus
// vectors exercise the same source kernel through the interpreter.
#[test]
#[ignore = "full gate: native million-byte reference"]
fn full_gate_sha256_million_a_native_reference() {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/sha256-million-native.hir");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let outcome = native::run_path_native(&path, &mut stdout, &mut stderr)
            .unwrap()
            .unwrap();
        assert!(matches!(outcome, RunOutcome::Exit(0)), "{outcome:?}");
        assert!(stdout.is_empty(), "{stdout:?}");
        assert!(stderr.is_empty(), "{}", String::from_utf8_lossy(&stderr));
    });
}

#[test]
fn unicode_properties_are_ordinary_source() {
    source_codec(
        "unicode-properties",
        "unicode",
        &[
            "is_whitespace",
            "is_letter",
            "is_digit",
            "simple_lower",
            "simple_upper",
        ],
    );
}

#[test]
fn unicode16_property_tables_regenerate() {
    assert!(
        std::process::Command::new("python3")
            .current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .args(["stdlib/unicode16/generate.py", "--check"])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn ascii_radix_is_ordinary_source() {
    source_codec("ascii-radix", "ascii", &["digit_value_radix"]);
}

#[test]
fn csv_is_ordinary_source() {
    source_codec(
        "csv",
        "csv",
        &["reader", "feed", "parse", "read_record", "write_record"],
    );
}

#[test]
fn cli_is_ordinary_source() {
    source_codec("cli", "cli", &["parse", "usage"]);
}
