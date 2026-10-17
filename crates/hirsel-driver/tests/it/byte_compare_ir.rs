//! Byte-key probes use the target's byte comparison instead of a scalar loop.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{OptLevel, Options, TargetSpec, build, emit_ir};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};

#[test]
fn byte_key_probes_use_a_bulk_comparison_on_both_targets() {
    for target in [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        with_compiler_stack(move || {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let source = "fn equal(left: Bytes, right: Bytes) -> bool { left == right }
fn less(left: Bytes, right: Bytes) -> bool { left < right }
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  failure.check(equal(b\"abc\", b\"abc\") && less(b\"abc\", b\"abd\"), TextInvariant())
  Ok(())
}
";
            let file = engine.sources.add("probes.hir", source.to_owned()).unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let input = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(input).unwrap();
            let ready = build(&validated, &Options::new(target)).unwrap();
            let ir = emit_ir(&ready, OptLevel::Speed).unwrap();
            for name in ["5equal", "4less"] {
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
                assert!(
                    body.contains("@memcmp(") || body.contains("@bcmp("),
                    "{name} must use a bulk byte comparison\n{body}"
                );
                if name == "5equal" {
                    assert!(
                        body.contains("load i16,") && body.contains("load i32,"),
                        "short equality must compare bounded overlapping words\n{body}"
                    );
                } else {
                    assert!(
                        !body.contains("load i8,"),
                        "ordering must use the target bulk comparison\n{body}"
                    );
                }
            }
        });
    }
}
