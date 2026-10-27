//! Checkpoint chunks preserve the exact quota tail and reject observable loops.
use super::*;

const LOOP: &str = include_str!("../../tests/fixtures/ahead_loop.ll");

fn optimized(source: &str) -> String {
    let context = Context::new();
    let module = Module::parse(&context, "ahead-chunks", source).unwrap();
    let machine = TargetMachine::new(&TargetSpec::x86_64_unknown_linux_gnu(), true).unwrap();
    module.set_target(&machine);
    module.verify().unwrap();
    module.run_passes("default<O3>", &machine).unwrap();
    module.verify().unwrap();
    module.text()
}

#[test]
fn private_recurrence_has_checkpoint_free_chunks_and_an_exact_tail() {
    let ir = optimized(LOOP);
    assert!(ir.contains("ahead.boundary"), "{ir}");
    assert!(ir.contains("ahead.chunk"), "{ir}");
    assert!(ir.contains("@hirsel_rt_v2_ahead_handoff"), "{ir}");
    let mut chunk = false;
    let mut blocks = 0;
    for line in ir.lines() {
        if !line.starts_with(' ') && line.contains(':') {
            chunk = line.split(':').next().unwrap().contains("ahead.chunk");
            blocks += usize::from(chunk);
        }
        if chunk {
            assert!(!line.contains(" call "), "{line}\n{ir}");
            assert!(
                !line.contains(" load ") && !line.contains(" store "),
                "{line}\n{ir}"
            );
        }
    }
    assert!(blocks > 0);
}

#[test]
fn observable_atomic_fuel_and_unbounded_loops_keep_every_checkpoint() {
    for source in [
        LOOP.replace(
            "%shift = lshr",
            "store volatile i64 %i, ptr %state\n %shift = lshr",
        ),
        LOOP.replace(
            "%shift = lshr",
            "%fuel = load atomic i64, ptr %state monotonic, align 8\n %shift = lshr",
        ),
        LOOP.replace("%done = icmp eq i64 %i, %n", "%done = icmp eq i64 %x, %n"),
        LOOP.replace("i32 1)", "i32 3)"),
        LOOP.replace("%next = add i64 %i, 1", "%next = add i64 %i, 2"),
        LOOP.replace("[%remaining, %body]", "[%changed, %body]")
            .replace(
                "%next = add i64 %i, 1",
                "%changed = add i64 %remaining, 1\n %next = add i64 %i, 1",
            ),
    ] {
        let ir = optimized(&source);
        assert!(!ir.contains("ahead.boundary"), "{ir}");
        assert!(ir.contains("call void @hirsel_rt_v2_ahead_handoff"), "{ir}");
    }
}
