//! MEM-3 checks disappear only when the independently validated type bounds them.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{OptLevel, Options, TargetSpec, build, emit_ir};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};

#[test]
fn bounded_structural_types_need_no_native_preflight() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("bounded.hir", "pub fn main() -> bool {\n  let a = [1u64, 2u64]\n  let b = [1u64, 2u64]\n  a == b\n}\n".to_owned()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        for target in TargetSpec::executed() {
            let ready = build(&validated, &Options::new(target)).unwrap();
            let ir = emit_ir(&ready, OptLevel::None).unwrap();
            assert!(!ir.contains("@hirsel_rt_v2_depth_preflight("), "{ir}");
        }
    });
}
