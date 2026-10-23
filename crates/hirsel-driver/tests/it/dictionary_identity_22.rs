//! Manifest fixture shape adapted from rust-analyzer nameres/tests/mod_resolution.rs:1-96
//! (3131751e, MIT OR Apache-2.0). Expected-field instantiation vectors follow
//! Koka Type/Unify.hs:177-214,374-404 (198fc3bb, Apache-2.0), with Hirsel's
//! GEN-6 brands and type/size actuals, rather than reexports or effect rows.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_tir::ValueKind;
use hirsel_types::{GenericArgument, Primitive, Size, TypeKind};
use std::path::{Path, PathBuf};

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/GEN")
        .join(name)
}

fn check_brand(name: &str, rejected: bool) {
    with_compiler_stack(|| {
        let mut program = load_program(&corpus(name)).expect("manifest setup must succeed");
        let output = program.check(LoweringPolicy::Comptime).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        let errors: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|d| d.severity == hirsel_diag::Severity::Error)
            .collect();
        eprintln!("DICTIONARY_IDENTITY_22_SOURCE {name}: {errors:?}");
        if rejected {
            assert_eq!(errors.len(), 1, "{name}: {errors:?}");
            assert_eq!(errors[0].rule.as_str(), "PROV-1", "{errors:?}");
            let site = program.engine.sources.resolve(errors[0].primary);
            let source = program.engine.sources.file(site.file).unwrap();
            assert_eq!(source.path.file_name().unwrap(), name);
            let line = source.text()[..site.start as usize]
                .bytes()
                .filter(|b| *b == b'\n')
                .count()
                + 1;
            assert_eq!(
                line,
                if name.contains("version") { 12 } else { 10 },
                "{errors:?}"
            );
        } else {
            assert!(errors.is_empty(), "{name}: {errors:?}");
            // Reachable count/main, including the alias-branded operand, reaches V.
            program.check_for_execution().unwrap();
            let input = program.engine.export_native_input().unwrap();
            program.engine.import_native_input(input).unwrap();
        }
    });
}

#[test]
fn terminal_aliases_across_two_packages_accept() {
    check_brand("GEN-6-package-identity-run.hir", false);
}

#[test]
fn identical_distinct_constructions_reject() {
    check_brand("GEN-6-package-identity-distinct-bad.hir", true);
}

#[test]
fn same_spelling_in_another_version_rejects() {
    check_brand("GEN-6-package-identity-version-bad.hir", true);
}

#[test]
fn source_alias_cycle_is_a_language_rejection() {
    with_compiler_stack(|| {
        let mut program = load_program(&corpus("GEN-6-package-cycle-bad.hir"))
            .expect("acyclic manifest graph must load");
        let output = program.check(LoweringPolicy::Comptime).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        let errors: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|d| d.severity == hirsel_diag::Severity::Error)
            .collect();
        assert!(!errors.is_empty());
        for error in errors {
            assert_eq!(error.rule.as_str(), "GEN-6", "{error:?}");
            assert!(error.message.contains("cycle"), "{error:?}");
            assert!(error.message.contains("cycle@1/FIRST"), "{error:?}");
            assert!(error.message.contains("cycle@1/SECOND"), "{error:?}");
        }
    });
}

#[test]
fn named_callable_actuals_survive_package_aliases() {
    with_compiler_stack(|| {
        let mut program = load_program(&corpus("dictionary-identity-22/return.json")).unwrap();
        program.check_for_execution().unwrap();
        let input = program.engine.export_native_input().unwrap();
        let definition = |package: &str, name: &str| {
            DefPath(hirsel_syntax::ItemPath {
                package: hirsel_syntax::PackageId {
                    name: package.into(),
                    version: "1".into(),
                },
                modules: vec![],
                name: name.into(),
            })
        };
        // Each alias must retain the original body plus its independent actual.
        for (package, name, target) in [
            ("dict", "LEFT", "left"),
            ("dict", "RIGHT", "right"),
            ("bridge", "LEFT", "left"),
            ("relay", "LEFT", "left"),
        ] {
            let value = &input.constants[&definition(package, name)];
            let ValueKind::Construct { fields, .. } = &value.root.kind else {
                panic!("record")
            };
            assert_eq!(fields.len(), 4);
            for (index, field) in fields.iter().enumerate() {
                let ValueKind::FunctionReference {
                    definition: actual,
                    arguments,
                } = &field.kind
                else {
                    panic!("named function")
                };
                assert_eq!(
                    *actual,
                    definition("dict", if index < 2 { target } else { "bytes" })
                );
                assert_eq!(arguments.len(), 1);
                match (&arguments[0], index) {
                    (GenericArgument::Type(ty), 0 | 1) => assert_eq!(
                        program.engine.types().get(*ty).unwrap().kind,
                        TypeKind::Primitive(if index == 0 {
                            Primitive::U32
                        } else {
                            Primitive::U64
                        })
                    ),
                    (GenericArgument::Size(Size::Constant(n)), 2 | 3) => {
                        assert_eq!(*n, index as u64)
                    }
                    actual => panic!("wrong actual: {actual:?}"),
                }
            }
        }
        program.engine.import_native_input(input).unwrap();
    });
}
