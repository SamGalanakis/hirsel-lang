//! Bounded recognition controls; overflow-only IR specimens are never runtime
//! language tests. Ordinary source parity is checked independently.

use hirsel_lir::emission::OptLevel;
use hirsel_lir::layout_types::TargetSpec;
use hirsel_llvm::compile_ir;
use object::read::{Object as _, ObjectSection as _};

fn recurrence(mask: &str, low_bits: u32) -> String {
    format!(
        r#"
target triple = "x86_64-unknown-linux-gnu"
define i32 @independent_reduction(i32 %initial) {{
entry:
  br label %loop
loop:
  %state = phi i32 [ %initial, %entry ], [ %next, %loop ]
  %index = phi i32 [ 0, %entry ], [ %index.next, %loop ]
  %bit = and i32 %state, {low_bits}
  %mask = {mask}
  %poly = and i32 %mask, 79764919
  %shifted = lshr i32 %state, 1
  %next = xor i32 %shifted, %poly
  %index.next = add i32 %index, 1
  %more = icmp ult i32 %index.next, 8
  br i1 %more, label %loop, label %exit
exit:
  ret i32 %next
}}
!0 = !{{}}
"#
    )
}

fn has_table(ir: &str) -> bool {
    let artifact = compile_ir(
        ir,
        &TargetSpec::x86_64_unknown_linux_gnu(),
        OptLevel::Default,
    )
    .expect("verified module");
    let file = object::File::parse(artifact.bytes.as_slice()).unwrap();
    file.sections()
        .any(|section| section.name().unwrap().contains("rodata") && section.size() >= 256 * 4)
}

#[test]
fn a_proven_unsigned_wrapping_mask_exposes_a_table() {
    assert!(has_table(&recurrence(
        "sub i32 0, %bit, !hirsel.unsigned.wrapping.sub !0",
        1
    )));
}

#[test]
fn unknown_subtraction_provenance_stays_scalar() {
    assert!(!has_table(&recurrence("sub i32 0, %bit", 1)));
}

#[test]
fn a_non_boolean_mask_stays_scalar() {
    assert!(!has_table(&recurrence(
        "sub i32 0, %bit, !hirsel.unsigned.wrapping.sub !0",
        3
    )));
}

#[test]
fn overflow_flagged_ir_is_not_a_defined_wrapping_source_witness() {
    for flag in ["nuw", "nsw"] {
        assert!(!has_table(&recurrence(
            &format!("sub {flag} i32 0, %bit, !hirsel.unsigned.wrapping.sub !0"),
            1
        )));
    }
}
