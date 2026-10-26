//! Admission counters do not need invocation-site storage around ordinary work.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{OptLevel, Options, TargetSpec, build, emit_ir};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};

fn counter_scope(source: &str) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("scope.hir", source.into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        for target in TargetSpec::executed() {
            let ready = build(&validated, &Options::new(target)).unwrap();
            let ir = emit_ir(&ready, OptLevel::None).unwrap();
            for symbol in if source.contains("fn size") {
                &["4main", "4size"][..]
            } else {
                &["4main"][..]
            } {
                let body = ir
                    .lines()
                    .skip_while(|line| {
                        !(line.starts_with("define")
                            && line.contains(symbol)
                            && !line.contains("@_H1T"))
                    })
                    .take_while(|line| *line != "}")
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(!body.is_empty(), "{ir}");
                assert!(body.contains("icmp uge i16"), "{body}");
                assert!(body.contains("store i16"), "{body}");
                assert!(
                    !body.contains("@hirsel_rt_v2_depth_enter_at_site("),
                    "{body}"
                );
                assert!(!body.contains("@hirsel_rt_v2_depth_leave("), "{body}");
            }
        }
    });
}

#[test]
fn primitive_scope_uses_only_the_counter() {
    counter_scope("pub fn main()->Bytes {bytes.slice(b\"abcdef\", 1u64, 4u64)}\n");
}

#[test]
fn owned_argument_cleanup_keeps_its_counter_without_a_site_entry() {
    counter_scope(
        "fn size(x:String)->u64 {bytes.len(bytes.slice(string.as_bytes(x),0u64,2u64))}\npub fn main()->u64 {size(\"abcdef\")}\n",
    );
}
