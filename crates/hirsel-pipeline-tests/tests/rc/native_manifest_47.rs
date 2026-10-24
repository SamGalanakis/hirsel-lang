//! Source-derived native tests need reviewed runtime capability authority.
use crate::rc_support::{compile, manifest, target};

#[test]
fn runtime_byte_leaf_uses_reviewed_manifest() {
    let program = compile(
        r#"pub fn main() -> Result[(), u64] {
  let data = bytes.slice(b"abc", 0u64, 1u64)
  Error(bytes.len(data))
}"#,
    );
    hirsel_driver::native::build_program(
        &program,
        manifest(&program),
        &hirsel_driver::native::Options::new(target()),
    )
    .unwrap();
}
