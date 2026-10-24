//! Source admissions must survive the native formatting path.
use crate::rc_support::{compile, target, unit_named};

#[test]
fn appended_format_keeps_reserve_and_finish_invocations() {
    let program = compile(
        r#"pub fn main() -> Result[(), u64] {
  let mut output: List[u8] = []
  std@1/text.append(mut output, f"{42u64},")
  Error(list.len(output))
}"#,
    );
    let lowered = hirsel_rc::import::lower(&program, &target()).unwrap();
    let unit = unit_named(&lowered, "main");
    let reference = program.operation_registry();
    let rows = &reference.invocations[&unit.header.definition];
    assert!(
        rows.iter()
            .any(|r| r.callee.as_ref().is_some_and(|p| p.is_core("list.reserve")))
    );
    assert!(rows.iter().any(|r| {
        r.callee
            .as_ref()
            .is_some_and(|p| p.is_core("$intrinsic.TextFinish"))
    }));
    assert_eq!(hirsel_lir::invocations::check(unit, rows, 0), Ok(()));
}
