//! Byte repetition must retain the reserve-and-fill loop's bulk lowering.
use hirsel_driver::{load_program_source, native};
use hirsel_engine::with_compiler_stack;
use std::path::Path;

#[test]
fn plain_filled_bulk_initializes_on_both_targets() {
    for target in [
        native::TargetSpec::x86_64_unknown_linux_gnu(),
        native::TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        with_compiler_stack(move || {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir");
            let source = r#"
fn bytes(n: u64, value: u8) -> List[u8] { list.filled(n, value) }
fn signed(n: u64, value: i8) -> List[i8] { list.filled(n, value) }
fn scores(n: u64) -> List[i32] { list.filled(n, 2147483647i32) }
fn parents(n: u64) -> List[i32] { list.filled(n, -1i32) }
fn unsigned(n: u64) -> List[u32] { list.filled(n, 4294967295u32) }
pub fn main() -> Result[(), ()] {
  let a = bytes(4096, 85u8)
  let b = signed(4096, -7i8)
  failure.check(a[4095] == 85u8 && b[4095] == -7i8, Bounds())
  failure.check(scores(4096)[4095] == 2147483647i32, Bounds())
  failure.check(parents(4096)[4095] == -1i32, Bounds())
  failure.check(unsigned(4096)[4095] == 4294967295u32, Bounds())
  Ok(())
}
"#;
            let mut loaded = load_program_source(&path, Some(source)).unwrap();
            loaded.check_for_execution().unwrap();
            let input = loaded.engine.export_native_input().unwrap();
            let validated = loaded.engine.import_native_input(input).unwrap();
            let ready = native::build(&validated, &native::Options::new(target)).unwrap();
            let ir = native::emit_ir(&ready, native::OptLevel::Speed).unwrap();
            // Inspect dynamic callers: the small filled implementation can
            // inline after its capacity check has been removed.
            for name in ["5bytes", "6signed", "6scores", "7parents", "8unsigned"] {
                let body = ir
                    .lines()
                    .skip_while(|line| {
                        !(line.starts_with("define ")
                            && line.contains(name)
                            && !line.contains("@_H1T"))
                    })
                    .take_while(|line| *line != "}")
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(!body.is_empty(), "{name} is defined\n{ir}");
                if name == "6scores" {
                    assert!(
                        body.lines()
                            .any(|line| line.contains("store <") && line.contains(" x i32>")),
                        "non-byte-pattern seeds must vectorize\n{body}"
                    );
                } else {
                    assert!(
                        body.contains("call void @llvm.memset."),
                        "repeated byte patterns must use memset\n{body}"
                    );
                }
            }
        });
    }
}
