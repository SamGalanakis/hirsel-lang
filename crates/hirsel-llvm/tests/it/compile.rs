//! In-process checks of the pinned binding and the textual smoke path: no linking, no
//! execution (those are in `native.rs`).

use hirsel_lir::emission::OptLevel;
use hirsel_lir::layout_types::TargetSpec;
use hirsel_lir::object::ObjectFile;
use hirsel_llvm::{
    CodegenError, PINNED_LLVM, VerifyStage, check_target, compile_ir, loaded_llvm_version,
};

const SMOKE: &str = include_str!("../../../../tools/native-toolchain/fixtures/smoke.ll");

const SMOKE_EXPORTS: [&str; 9] = [
    "smoke_add_i64",
    "smoke_call_host",
    "smoke_div_f64",
    "smoke_div_i64",
    "smoke_mul_add",
    "smoke_mul_i128",
    "smoke_mul_then_add",
    "smoke_packed_sum",
    "smoke_shl_u64",
];

fn x86() -> TargetSpec {
    TargetSpec::x86_64_unknown_linux_gnu()
}

fn aarch64() -> TargetSpec {
    TargetSpec::aarch64_unknown_linux_gnu()
}

/// Baseline x86-64 has no FMA instruction, so `llvm.fma` is a call to the C library's
/// correctly rounded `fma`; AArch64 fuses in hardware.
fn smoke_imports(target: &TargetSpec) -> &'static [&'static str] {
    if target.triple.starts_with("x86_64") {
        &["fma", "smoke_host_scale"]
    } else {
        &["smoke_host_scale"]
    }
}

#[test]
fn a_target_spec_that_disagrees_with_llvm_is_rejected() {
    let mut wrong = aarch64();
    wrong.llvm_data_layout = "e-m:e-i8:8:32-i16:16:32-i64:64-i128:128-n32:64-S128-Fn32".into();
    match check_target(&wrong) {
        Err(CodegenError::DataLayout { what, .. }) => assert_eq!(what, "layout string"),
        other => panic!("expected a DataLayout mismatch, got {other:?}"),
    }
    let mut wrong = x86();
    wrong.alignments.i128 = 8;
    match check_target(&wrong) {
        Err(CodegenError::DataLayout { what, .. }) => assert_eq!(what, "I128 size and alignment"),
        other => panic!("expected an alignment mismatch, got {other:?}"),
    }
    let mut wrong = x86();
    wrong.pointer_bytes = 4;
    match check_target(&wrong) {
        Err(CodegenError::DataLayout { what, .. }) => assert_eq!(what, "pointer bytes"),
        other => panic!("expected a pointer width mismatch, got {other:?}"),
    }
}

#[test]
fn the_smoke_fixture_exports_and_imports_its_manifest_on_every_target() {
    for target in TargetSpec::executed() {
        for opt in [OptLevel::None, OptLevel::Speed] {
            let object = compile_ir(SMOKE, &target, opt).unwrap();
            assert_eq!(object.exports, SMOKE_EXPORTS, "{} {opt:?}", target.triple);
            assert_eq!(
                object.imports,
                smoke_imports(&target),
                "{} {opt:?}",
                target.triple
            );
            // Every import is referenced by a relocation.
            for import in &object.imports {
                assert!(
                    object.relocations.iter().any(|r| r.symbol == *import),
                    "{import}"
                );
            }
        }
    }
}

#[test]
fn malformed_ir_is_a_parse_error() {
    let error = compile_ir("define i8 @f( {", &x86(), OptLevel::None).unwrap_err();
    assert!(matches!(error, CodegenError::Parse { .. }), "{error:?}");
}

/// Parses, but `%x` is defined in a block that does not dominate its use.
const NON_DOMINATING: &str = "
define i32 @f(i32 %a) {
entry:
  br label %use
use:
  ret i32 %x
dead:
  %x = add i32 %a, 1
  br label %use
}
";

#[test]
fn the_verifier_runs_before_optimization() {
    for opt in [OptLevel::None, OptLevel::Speed] {
        match compile_ir(NON_DOMINATING, &aarch64(), opt) {
            Err(CodegenError::Verify { stage, message }) => {
                assert_eq!(stage, VerifyStage::BeforeOptimization);
                assert!(
                    message.contains("Instruction does not dominate all uses"),
                    "{message}"
                );
            }
            other => panic!("expected a verifier error, got {other:?}"),
        }
    }
    let twin = NON_DOMINATING.replace("ret i32 %x", "ret i32 %a");
    compile_ir(&twin, &aarch64(), OptLevel::Speed).unwrap();
}

#[test]
fn a_module_for_another_target_is_rejected() {
    let foreign = format!("target triple = \"{}\"\n{SMOKE}", aarch64().triple);
    match compile_ir(&foreign, &x86(), OptLevel::None) {
        Err(CodegenError::Triple { expected, actual }) => {
            assert_eq!(expected, x86().triple);
            assert_eq!(actual, "aarch64-unknown-linux-gnu");
        }
        other => panic!("expected a triple mismatch, got {other:?}"),
    }
    compile_ir(&foreign, &aarch64(), OptLevel::None).unwrap();

    let layout = format!(
        "target datalayout = \"{}\"\n{SMOKE}",
        aarch64().llvm_data_layout
    );
    match compile_ir(&layout, &x86(), OptLevel::None) {
        Err(CodegenError::DataLayout {
            expected, actual, ..
        }) => {
            assert_eq!(expected, x86().llvm_data_layout);
            assert_eq!(actual, aarch64().llvm_data_layout);
        }
        other => panic!("expected a DataLayout mismatch, got {other:?}"),
    }
    compile_ir(&layout, &aarch64(), OptLevel::None).unwrap();
}
