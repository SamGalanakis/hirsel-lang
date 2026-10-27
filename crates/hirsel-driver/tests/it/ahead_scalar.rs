use hirsel_driver::{compiler_adapters, native};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};
use hirsel_lir::ahead::{Refusal, ScalarGraph, scalar_graph};
use hirsel_syntax::ContentHash;

fn analyze(source: &str, name: &str) -> Result<ScalarGraph, Refusal> {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("ahead.hir", source.to_owned()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let ready = native::build(&validated, &native::Options::new(target.clone())).unwrap();
        let root = ready
            .units
            .iter()
            .find(|u| u.unit().header.definition.0.name == name)
            .unwrap()
            .unit()
            .header
            .instance;
        scalar_graph(
            &ready.units.iter().collect::<Vec<_>>(),
            &ready.table,
            &target,
            root,
            false,
            ContentHash::of_bytes(b"test helper identity"),
        )
    })
}

#[test]
fn scalar_cfg_loop_keeps_original_fuel_points() {
    for source in [
        "fn main()->u64 {let mut x=1u64; for i in 0u64..64u64 {x=int.wrapping_add(int.wrapping_mul(x ^ (x >> 7),0x9e3779b97f4a7c15u64),i)}; x}",
        "fn main()->f64 {let mut x=1.0f64; for i in 0u64..64u64 {x=x*1.0000000000000002f64+0.0000000000000001f64}; x}",
    ] {
        let graph = analyze(source, "main").unwrap();
        assert!(
            graph
                .bodies
                .values()
                .any(|b| b.fuel_points.iter().filter(|p| **p).count() > 1)
        );
    }
}

#[test]
fn recursive_scalar_scc_is_refused() {
    let error = analyze("fn spin(x:u64)->u64 {if x==0u64 {0u64} else {1u64+spin(x-1u64)}}\nfn main()->u64 {spin(2u64)}", "main").unwrap_err();
    assert!(error.reason.contains("recursive"), "{error:?}");
}

#[test]
fn opaque_indirect_call_is_refused() {
    let error = analyze(
        "fn apply(f:fn(u64)->u64 uses {})->u64 uses {f} {f(1u64)}\nfn main()->u64 {0u64}",
        "apply",
    )
    .unwrap_err();
    assert!(
        error.reason.contains("scalar") || error.reason.contains("indirect"),
        "{error:?}"
    );
}

#[test]
fn allocating_result_is_refused() {
    assert!(analyze("fn main()->List[u64] {[1u64,2u64]}", "main").is_err());
}

#[test]
fn input_sized_comparison_and_resource_cleanup_are_refused() {
    for (source, name) in [
        (
            "fn compare(a:Bytes,b:Bytes)->bool {a==b}\nfn main()->u64 {0u64}",
            "compare",
        ),
        (
            "resource type R {n:u64}\ndrop(r) {let _=10u64/r.n;()}\nfn consume(take r:R)->u64 {r.n}\nfn main()->u64 {consume(R(n:2u64))}",
            "consume",
        ),
    ] {
        assert!(analyze(source, name).is_err());
    }
}

#[test]
fn scalar_alternate_loop_ir() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let source = "fn main(seed:u64,count:u64)->u64 uses {} {let mut x=seed; for i in 0u64..count {x=int.wrapping_add(int.wrapping_mul(x ^ (x >> 7),0x9e3779b97f4a7c15u64),i)}; x}";
        let file = engine.sources.add("chunk.hir", source.to_owned()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let program = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let units = program.units.iter().collect::<Vec<_>>();
        let inputs = hirsel_lir::emission::EmitInputs {
            ahead: None,
            continuation: None,
            alternate: true,
            index: None,
            verify_backend: true,
            input: &program.input,
            target: &program.target,
            layouts: &program.table,
            keys: &program.keys,
            manifest: &program.manifest,
            statics: &program.statics,
            inline_instances: &program.inline_instances,
            available_externally: &Default::default(),
            callbacks: false,
            tracking: program.tracking(),
            foreign: &program.foreign_dispatch,
            opt: hirsel_lir::emission::OptLevel::Speed,
            measurement: false,
            fuel: false,
        };
        let after = hirsel_llvm::emit_optimized_ir(&units, &inputs).unwrap();
        assert!(after.contains("ahead.boundary"), "{after}");
    });
}
