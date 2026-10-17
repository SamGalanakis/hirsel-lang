//! A nonfinal release of a slice owner needs only its task-local count word.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{OptLevel, Options, TargetSpec, build, emit_ir};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};

#[test]
fn erased_owners_decrement_shared_counts_before_the_runtime_fallback() {
    for target in [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        with_compiler_stack(move || {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let source = "fn consume(take data: Bytes) -> () {}
pub fn main() -> Result[(), ()] { consume(b\"a\")
  Ok(()) }
";
            let file = engine
                .sources
                .add("release.hir", source.to_owned())
                .unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let input = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(input).unwrap();
            let ready = build(&validated, &Options::new(target)).unwrap();
            let ir = emit_ir(&ready, OptLevel::Speed).unwrap();
            let body = ir
                .lines()
                .skip_while(|line| {
                    !(line.starts_with("define ")
                        && line.contains("7consume")
                        && !line.contains("@_H1T"))
                })
                .take_while(|line| *line != "}")
                .collect::<Vec<_>>()
                .join("\n");
            assert!(body.contains("@hirsel_rt_v2_release("), "{body}");
            assert!(
                body.contains("load i32,") && body.contains("store i32"),
                "shared releases must use the count word before the final-credit fallback\n{body}"
            );
        });
    }
}
