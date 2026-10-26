//! Fixed inline value construction cannot observe a caller's admission counter.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{OptLevel, Options, TargetSpec, build, emit_ir};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};

#[test]
fn inline_record_callee_uses_only_a_room_guard() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("leaf.hir", "fn pair(x:f64,y:f64)->(f64,f64) {(x,y)}\npub fn main()->(f64,f64) {pair(1.0,2.0)}\n".into()).unwrap();
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
                        && line.contains("4main")
                        && !line.contains("@_H1T"))
                })
                .take_while(|line| *line != "}")
                .collect::<Vec<_>>()
                .join("\n");
            assert!(!body.is_empty(), "{ir}");
            assert!(body.contains("icmp uge i16"), "{body}");
            assert!(!body.contains("store i16"), "{body}");
            assert!(
                !body.contains("@hirsel_rt_v2_depth_enter_at_site("),
                "{body}"
            );
            assert!(!body.contains("@hirsel_rt_v2_depth_leave("), "{body}");
        }
    });
}

#[test]
fn fixed_construction_proof_excludes_box_allocation() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add(
                "boxed.hir",
                "fn boxed(x:f64)->box f64 {box x}\npub fn main()->box f64 {boxed(1.0)}\n".into(),
            )
            .unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        for target in TargetSpec::executed() {
            let ready = build(&validated, &Options::new(target)).unwrap();
            let boxed = ready
                .units
                .iter()
                .find(|u| u.unit().header.definition.0.name == "boxed")
                .unwrap();
            assert!(!hirsel_lir::invocations::room::depth_leaf(
                boxed.unit(),
                &ready.table
            ));
        }
    });
}
