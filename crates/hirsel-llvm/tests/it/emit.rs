//! In-process emission of validated Ready units: receipts, failure codes, the object
//! manifest and owned, reproducible artifacts. Linking and execution are in `native.rs`.

use crate::support;

use hirsel_core_contract::Manifest;
use hirsel_lir::emission::{EmitInputs, FailureCodes, OptLevel, function_thunk_name, symbol_name};
use hirsel_lir::layout_types::TargetSpec;
use hirsel_lir::stage::{InputIdentity, ObjectArtifact, ObjectSymbolKind};
use hirsel_llvm::{CodegenError, emit_object};
use support::programs::*;
use support::*;

fn x86() -> TargetSpec {
    TargetSpec::x86_64_unknown_linux_gnu()
}

fn inputs<'a>(program: &'a Program, input: &'a InputIdentity, opt: OptLevel) -> EmitInputs<'a> {
    static EMPTY_INLINE: std::collections::BTreeSet<hirsel_syntax::ContentHash> =
        std::collections::BTreeSet::new();
    EmitInputs {
        index: None,
        verify_backend: true,
        input,
        target: &program.table.target,
        layouts: &program.table.table,
        keys: &program.table.keys,
        manifest: &program.manifest,
        statics: &program.statics,
        inline_instances: &EMPTY_INLINE,
        available_externally: &EMPTY_INLINE,
        callbacks: false,
        foreign: &hirsel_lir::emission::NO_FOREIGN,
        tracking: false,
        opt,
        measurement: false,
        fuel: false,
    }
}

#[test]
fn the_scalar_program_exports_every_case_on_every_target() {
    let input = input();
    for target in TargetSpec::executed() {
        let program = scalar_program(&target);
        for opt in [OptLevel::None, OptLevel::Speed] {
            let object = emit_object(&program.units(), &inputs(&program, &input, opt))
                .unwrap_or_else(|e| panic!("{}: {e}", target.triple));
            // Each definition and its function-reference thunk.
            let mut expected: Vec<String> = scalar_cases()
                .iter()
                .flat_map(|c| {
                    let (definition, instance) = (path(c.name), instance(c.name));
                    [
                        symbol_name(&definition, Some(&instance)),
                        function_thunk_name(&definition, Some(&instance)),
                    ]
                })
                .collect();
            expected.sort();
            let exports: Vec<String> = object.exports.iter().map(|s| s.name.clone()).collect();
            assert_eq!(exports, expected);
            assert!(
                object
                    .exports
                    .iter()
                    .all(|s| s.kind == ObjectSymbolKind::Function
                        && s.definition.is_some()
                        && s.abi.is_some())
            );
            // Failure starts the runtime's failure state with a rule's code from its glue
            // cell; i128 division and float remainder are toolchain routines; nothing else
            // is imported.
            for import in &object.imports {
                let runtime = import.name == "hirsel_rt_v2_failure_begin";
                let cell = import.name.starts_with("hirsel.fail.");
                assert!(
                    runtime
                        || cell
                        || hirsel_lir::emission::TOOLCHAIN_IMPORTS.contains(&import.name.as_str()),
                    "{}",
                    import.name
                );
                assert_eq!(import.kind == ObjectSymbolKind::Runtime, runtime);
                assert_eq!(import.kind == ObjectSymbolKind::Data, cell);
            }
            assert!(!object.relocations.is_empty());
        }
    }
}

/// The artifact owns its bytes and metadata, names its inputs, and is reproducible.
#[test]
fn the_artifact_is_owned_and_reproducible() {
    fn owned<T: Send + Sync + 'static>(_: &T) {}
    let input = input();
    let emit = || -> ObjectArtifact {
        let program = scalar_program(&x86());
        emit_object(&program.units(), &inputs(&program, &input, OptLevel::Speed)).unwrap()
    };
    let first = emit();
    owned(&first);
    let second = emit();
    assert_eq!(first, second, "emission is deterministic");
    let program = scalar_program(&x86());
    assert_eq!(first.target, x86().digest());
    assert_eq!(first.reproducibility.toolchain, "llvm 21.1.8");
    assert_eq!(&first.bytes[..4], b"\x7fELF");
}

/// Each binding a receipt carries is checked against the inputs emission was given. Every
/// rejection has an accepted twin: the same units with the inputs they were validated
/// against.
#[test]
fn a_receipt_for_other_inputs_is_rejected() {
    let input = input();
    let program = scalar_program(&x86());
    let units = program.units();
    let valid = inputs(&program, &input, OptLevel::None);
    emit_object(&units, &valid).expect("the accepted twin");

    let problem = |inputs: &EmitInputs<'_>| match emit_object(&units, inputs) {
        Err(CodegenError::Receipt { problem, .. }) => problem,
        other => panic!("expected a receipt rejection, got {other:?}"),
    };

    // Unrelated rows and front-end snapshots do not change this unit's proof.
    // Keep the original LayoutKeys deliberately: the receipt reads actual rows.
    let mut table = program.table.table.clone();
    table.layouts.push(table.layouts[0].clone());
    emit_object(
        &units,
        &EmitInputs {
            layouts: &table,
            ..valid.clone()
        },
    )
    .expect("unrelated layouts do not affect a unit");
    let manifest = Manifest::skeleton([]);
    emit_object(
        &units,
        &EmitInputs {
            manifest: &manifest,
            ..valid.clone()
        },
    )
    .expect("these scalar units reference no manifest rows");
    let reached = units[0].unit().types[0].layout;
    table.layouts[reached.0 as usize].align += 1;
    assert_eq!(
        problem(&EmitInputs {
            layouts: &table,
            ..valid.clone()
        }),
        "the receipt names other checked inputs"
    );
    let mut table = program.table.table.clone();
    table.fn_abis[units[0].unit().header.abi.0 as usize].caller_site = true;
    assert_eq!(
        problem(&EmitInputs {
            layouts: &table,
            ..valid.clone()
        }),
        "the receipt names other checked inputs"
    );

    let aarch64 = TargetSpec::aarch64_unknown_linux_gnu();
    assert_eq!(
        problem(&EmitInputs {
            target: &aarch64,
            ..valid.clone()
        }),
        "the receipt names another target"
    );

    let other = InputIdentity {
        interface_version: input.interface_version,
        digest: hash("another input"),
    };
    emit_object(
        &units,
        &EmitInputs {
            index: None,
            verify_backend: true,
            input: &other,
            ..valid.clone()
        },
    )
    .expect("a program input digest is not unit input evidence");

    // A unit sealed against an AArch64 table cannot be emitted with the x86-64 table.
    let foreign = scalar_program(&aarch64);
    let mixed: Vec<_> = vec![units[0], foreign.units()[0]];
    assert_eq!(
        match emit_object(&mixed, &valid) {
            Err(CodegenError::Receipt { problem, .. }) => problem,
            other => panic!("expected a receipt rejection, got {other:?}"),
        },
        "the receipt names another target"
    );
}

/// A unit names a failure rule by its glue cell, not by the code the program assigns it:
/// the object imports `hirsel.fail.<rule>.code` as data.
#[test]
fn a_failure_rule_is_a_glue_import() {
    let input = input();
    let program = scalar_program(&x86());
    let object = emit_object(&program.units(), &inputs(&program, &input, OptLevel::None)).unwrap();
    let cell = hirsel_lir::glue::failure_code_symbol("SEM-1");
    assert!(
        object
            .imports
            .iter()
            .any(|s| s.name == cell && s.kind == ObjectSymbolKind::Data),
        "{:?}",
        object.imports
    );
}

#[test]
fn two_units_with_one_link_name_are_rejected() {
    let input = input();
    let program = scalar_program(&x86());
    let units = program.units();
    match emit_object(
        &[units[0], units[0]],
        &inputs(&program, &input, OptLevel::None),
    ) {
        Err(CodegenError::DuplicateSymbol { name }) => {
            let case = &units[0].unit().header.definition.0.name;
            assert_eq!(name, symbol_name(&path(case), Some(&instance(case))));
        }
        other => panic!("expected a duplicate symbol, got {other:?}"),
    }
}

#[test]
fn a_forged_or_foreign_program_descriptor_assignment_is_rejected() {
    let program = heap_program(&x86());
    let codes = failure_codes();
    let keys = &program.table.keys;
    let glue = hirsel_lir::glue::UnitGlue::default();
    let valid = hirsel_lir::emission::GlueInputs {
        target: &program.table.target,
        layouts: &program.table.table,
        keys,
        registry: hirsel_lir::descriptor_registry::DescriptorRegistry::new(
            &program.table.table,
            &program.finalizers,
        ),
        finalizers: &program.finalizers,
        glue: &glue,
        failures: &codes,
        statics: &program.statics,
        callbacks: &[],
        foreign: &hirsel_lir::emission::NO_FOREIGN,
        aliasing_unstated: false,
    };
    assert!(valid.registry.layouts.len() > 2);
    let mut swapped = valid.clone();
    swapped.registry.layouts.swap(0, 1);
    let mut missing = valid.clone();
    missing.registry.layouts.pop();
    let mut duplicate = valid.clone();
    duplicate.registry.layouts[1] = duplicate.registry.layouts[0];
    let mut foreign = valid.clone();
    foreign.registry.target = TargetSpec::aarch64_unknown_linux_gnu().digest();
    let mut stale = valid.clone();
    stale.registry.identity = hash("stale registry");
    for forged in [swapped, missing, duplicate, foreign, stale] {
        assert!(
            matches!(hirsel_llvm::emit_glue(&forged), Err(CodegenError::Manifest { problem }) if problem.contains("descriptor registry"))
        );
    }
    hirsel_llvm::emit_glue(&valid).unwrap();
}

/// A uniqueness query must stay a count read, including at O0: it must never
/// acquire an opaque runtime call that clobbers surrounding sequence facts.
#[test]
fn uniqueness_queries_do_not_import_a_runtime_entry() {
    let input = input();
    for target in TargetSpec::executed() {
        let program = support::ready::ready_program(&target);
        for opt in [OptLevel::None, OptLevel::Speed] {
            let object = emit_object(&program.units(), &inputs(&program, &input, opt)).unwrap();
            assert!(
                object
                    .imports
                    .iter()
                    .all(|symbol| symbol.name != "hirsel_rt_v2_is_unique"),
                "a uniqueness query imported the runtime on {} at {opt:?}",
                target.triple
            );
        }
    }
}

#[test]
fn slice_storage_is_bound_even_when_only_the_slice_type_names_it() {
    use hirsel_lir::ir::ValueRepr;
    use hirsel_lir::layout_types::LayoutKind;
    use hirsel_syntax::PassingMode;
    use hirsel_types::Primitive;
    let target = x86();
    let mut schema = Schema::new();
    let text = schema.types.p(Primitive::String);
    let word = schema.types.p(Primitive::I64);
    schema.name("text", text);
    schema.signature("identity", &[(PassingMode::Borrow, word)], word);
    let table = schema.build(&target);
    let manifest = manifest();
    let mut u = Unit::new(&table, "identity", "identity");
    u.ty(ValueRepr::Aggregate, table.layout("text"));
    let value = u.param(table.scalar(hirsel_lir::layout_types::ScalarKind::I64));
    u.builder.ret(ENTRY, Some(value));
    let unit = seal(&table, &manifest, u.finish());
    let program = Program {
        table,
        manifest,
        units: vec![unit],
        statics: vec![],
        finalizers: Default::default(),
    };
    let input = input();
    let valid = inputs(&program, &input, OptLevel::None);
    let units = program.units();
    emit_object(&units, &valid).unwrap();
    let before =
        hirsel_lir::objects::ready_digest(units[0].unit(), &valid, "llvm", false, &[]).unwrap();
    let mut changed = program.table.table.clone();
    let storage = changed
        .layouts
        .iter()
        .position(|row| matches!(row.kind, LayoutKind::Sequence { .. }))
        .unwrap();
    changed.layouts[storage].key.owner_revision = Some(hash("changed storage"));
    // Keep producer LayoutKeys stale. The receipt still sees the changed raw
    // storage row although neither an operand nor the ABI directly names it.
    let stale = EmitInputs {
        layouts: &changed,
        ..valid.clone()
    };
    assert!(hirsel_lir::emission::check_receipt(units[0], &stale).is_err());
    let new_keys = hirsel_lir::LayoutKeys::new(&changed);
    let rebound = EmitInputs {
        keys: &new_keys,
        ..stale
    };
    assert_ne!(
        before,
        hirsel_lir::objects::ready_digest(units[0].unit(), &rebound, "llvm", false, &[]).unwrap()
    );
    let wrong = hirsel_lir::LayoutIndex::new(&program.table.table);
    assert!(
        hirsel_lir::emission::check_receipt(
            units[0],
            &EmitInputs {
                index: Some(&wrong),
                ..rebound
            }
        )
        .is_err()
    );
}
