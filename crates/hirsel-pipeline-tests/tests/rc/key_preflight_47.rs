//! A loop's projected key is evaluated once for preflight and transfer.
use crate::rc_support::{compile, dump, manifest, target, unit_named};

#[test]
fn borrowed_loop_key_is_preflighted_before_map_get() {
    check(
        r#"pub fn object(entries: Map[String, u64], keys: List[String]) -> () uses {} {
  for key in keys { let _ = map.get(entries, key) }
}
pub fn main() -> Result[(), ()] { object(map.empty(), []); Ok(()) }"#,
    );
}

#[test]
fn scalar_loop_key_keeps_its_preflight_address() {
    check(
        r#"pub fn object(entries: Map[u64, u64], keys: List[u64]) -> () uses {} {
  for key in keys { let _ = map.get(entries, key) }
}
pub fn main() -> Result[(), ()] { object(map.empty(), []); Ok(()) }"#,
    );
}

#[test]
fn copied_owned_loop_key_transfers_after_its_preflight() {
    check(
        r#"pub fn object(mut entries: Map[String, u64], take keys: List[String]) -> () uses {} {
  for key in keys { let _ = map.insert(mut entries, copy key, 0u64) }
}
pub fn main() -> Result[(), ()] {
  let mut entries: Map[String, u64] = map.empty()
  object(mut entries, [])
  Ok(())
}"#,
    );
}

fn check(source: &str) {
    let program = compile(source);
    let lowered = hirsel_rc::import::lower(&program, &target()).unwrap();
    let unit = unit_named(&lowered, "object");
    assert_eq!(
        hirsel_lir::structural_access::check_key_order(unit),
        Ok(()),
        "{}",
        dump(unit)
    );
    let mut missing = unit.clone();
    for instruction in &mut missing.instructions {
        if let hirsel_lir::ir::Inst::Hash { value, .. } = instruction {
            *value = unit.parameters[0];
        }
    }
    assert!(hirsel_lir::structural_access::check_key_order(&missing).is_err());
    hirsel_driver::native::build_program(
        &program,
        manifest(&program),
        &hirsel_driver::native::Options::new(target()),
    )
    .unwrap();
}
