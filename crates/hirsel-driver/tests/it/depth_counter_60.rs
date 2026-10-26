//! Validated source admission scopes lower to local counter updates on both targets.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{OptLevel, Options, TargetSpec, build, emit_ir};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};

#[test]
fn recursive_scalar_calls_use_counters_without_invocation_metadata() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("counter.hir", "fn descend(n:u64)->u64 { if n==0u64 {0u64} else {1u64+descend(n-1u64)} }\npub fn main()->u64 { descend(20u64) }\n".to_owned()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        for target in TargetSpec::executed() {
            let ready = build(&validated, &Options::new(target)).unwrap();
            let ir = emit_ir(&ready, OptLevel::None).unwrap();
            let body = ir
                .lines()
                .skip_while(|line| {
                    !(line.starts_with("define")
                        && line.contains("7descend")
                        && !line.contains("@_H1T"))
                })
                .take_while(|line| *line != "}")
                .collect::<Vec<_>>()
                .join("\n");
            assert!(!body.is_empty(), "{ir}");
            assert!(
                !body.contains("@hirsel_rt_v2_depth_enter_at_site("),
                "{body}"
            );
            assert!(!body.contains("@hirsel_rt_v2_depth_leave("), "{body}");
            assert!(body.contains("icmp uge i16"), "{body}");
            assert!(body.contains("store i16"), "{body}");
        }
    });
}
